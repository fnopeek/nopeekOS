/*
 * npk-camera: the guest's camera, a PipeWire video source.
 *
 * It shows up as a camera (media.class Video/Source, media.role Camera),
 * so xdg-desktop-portal's Camera portal offers it and a browser picks it
 * like a webcam. While a client streams from it, it asks the host for
 * the camera over the virtio-console port "npk.camera" (CAM_START, and
 * CAM_STOP when the stream ends) and turns each frame the host sends
 * (MJPEG, decoded with libjpeg-turbo, or YUY2) into YUY2. Without the
 * port it draws a moving test pattern.
 *
 * Follows PipeWire's src/examples/video-src.c: a driving output stream
 * with a timer that triggers each frame.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <setjmp.h>

#include <jpeglib.h>

#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>

#define WIDTH 1280
#define HEIGHT 720
#define FPS 30

/* Port framing (kernel/src/microvm/devices/virtio_console_pci.rs). */
#define CAM_START 0x81
#define CAM_STOP 0x82
#define CAM_FRAME 0x10
#define FORMAT_YUYV 1
#define FORMAT_MJPEG 3
#define FRAME_MAX (8 << 20)

struct data {
	struct pw_main_loop *loop;
	struct spa_source *timer;
	struct pw_stream *stream;
	struct spa_video_info_raw format;
	uint32_t frame;
	/* The host's camera port, -1 without; what came in of the frame being
	 * received; the newest frame as YUY2 and whether one arrived. */
	int port;
	struct spa_source *port_io;
	uint8_t *in;
	size_t in_len;
	uint8_t *yuyv;
	uint32_t yuyv_w, yuyv_h;
	int have_frame;
	uint64_t frames_in, bad_in;
};

/* /dev/vportNpM of the port named `name`, opened, or -1. */
static int open_port(const char *name)
{
	DIR *dir = opendir("/sys/class/virtio-ports");
	if (dir == NULL)
		return -1;
	struct dirent *e;
	int fd = -1;
	while (fd < 0 && (e = readdir(dir)) != NULL) {
		if (e->d_name[0] == '.')
			continue;
		char path[300], got[64] = {0};
		snprintf(path, sizeof(path), "/sys/class/virtio-ports/%s/name", e->d_name);
		FILE *f = fopen(path, "r");
		if (f == NULL)
			continue;
		if (fgets(got, sizeof(got), f) != NULL) {
			got[strcspn(got, "\n")] = 0;
			if (strcmp(got, name) == 0) {
				snprintf(path, sizeof(path), "/dev/%s", e->d_name);
				fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
			}
		}
		fclose(f);
	}
	closedir(dir);
	return fd;
}

static void port_send(struct data *d, uint8_t type, const uint8_t *payload, uint16_t n)
{
	if (d->port < 0)
		return;
	uint8_t msg[16];
	uint16_t len = n + 1;
	msg[0] = len & 0xff;
	msg[1] = len >> 8;
	msg[2] = type;
	memcpy(msg + 3, payload, n);
	if (write(d->port, msg, 3 + n) != 3 + n)
		fprintf(stderr, "npk-camera: port write: %s\n", strerror(errno));
}

/* JPEG's full-range YCbCr to the limited range a YUY2 consumer expects. */
static inline uint8_t y_limited(int v) { return (uint8_t)(16 + v * 219 / 255); }
static inline uint8_t c_limited(int v) { return (uint8_t)(128 + (v - 128) * 224 / 255); }

/* libjpeg's fatal errors return to decode_mjpeg instead of exiting. */
struct jpeg_err {
	struct jpeg_error_mgr mgr;
	jmp_buf back;
};

static void jpeg_fail(j_common_ptr cinfo)
{
	longjmp(((struct jpeg_err *)cinfo->err)->back, 1);
}

static void jpeg_quiet(j_common_ptr cinfo, int level) {}

/* A camera MJPEG frame into d->yuyv; libjpeg-turbo supplies the Huffman
 * tables a Motion-JPEG frame leaves out. A broken frame is dropped. */
static int decode_mjpeg(struct data *d, const uint8_t *src, size_t n)
{
	struct jpeg_decompress_struct cinfo;
	struct jpeg_err jerr;
	static uint8_t *row;
	static size_t row_len;
	cinfo.err = jpeg_std_error(&jerr.mgr);
	jerr.mgr.error_exit = jpeg_fail;
	jerr.mgr.emit_message = jpeg_quiet;
	if (setjmp(jerr.back)) {
		jpeg_destroy_decompress(&cinfo);
		return -1;
	}
	jpeg_create_decompress(&cinfo);
	jpeg_mem_src(&cinfo, src, n);
	if (jpeg_read_header(&cinfo, TRUE) != JPEG_HEADER_OK) {
		jpeg_destroy_decompress(&cinfo);
		return -1;
	}
	cinfo.out_color_space = JCS_YCbCr;
	cinfo.dct_method = JDCT_ISLOW;
	jpeg_start_decompress(&cinfo);
	uint32_t w = cinfo.output_width, h = cinfo.output_height;
	if (w != d->yuyv_w || h != d->yuyv_h || cinfo.output_components != 3) {
		jpeg_abort_decompress(&cinfo);
		jpeg_destroy_decompress(&cinfo);
		return -1;
	}
	if (row_len < (size_t)w * 3) {
		free(row);
		row_len = (size_t)w * 3;
		row = malloc(row_len);
	}
	while (cinfo.output_scanline < h) {
		uint32_t y = cinfo.output_scanline;
		JSAMPROW rows[1] = {row};
		jpeg_read_scanlines(&cinfo, rows, 1);
		uint8_t *out = d->yuyv + (size_t)y * w * 2;
		for (uint32_t x = 0; x + 1 < w; x += 2) {
			const uint8_t *p = row + x * 3;
			out[x * 2 + 0] = y_limited(p[0]);
			out[x * 2 + 1] = c_limited((p[1] + p[4]) / 2);
			out[x * 2 + 2] = y_limited(p[3]);
			out[x * 2 + 3] = c_limited((p[2] + p[5]) / 2);
		}
	}
	jpeg_finish_decompress(&cinfo);
	jpeg_destroy_decompress(&cinfo);
	return 0;
}

static void on_frame(struct data *d, const uint8_t *f, size_t n)
{
	if (n < 8 || d->yuyv == NULL)
		return;
	uint32_t w = f[0] | f[1] << 8, h = f[2] | f[3] << 8, fmt = f[4];
	const uint8_t *data = f + 8;
	size_t len = n - 8;
	int ok = -1;
	if (fmt == FORMAT_MJPEG) {
		ok = decode_mjpeg(d, data, len);
	} else if (fmt == FORMAT_YUYV && w == d->yuyv_w && h == d->yuyv_h && len >= (size_t)w * h * 2) {
		memcpy(d->yuyv, data, (size_t)w * h * 2);
		ok = 0;
	}
	if (ok == 0) {
		d->have_frame = 1;
		d->frames_in++;
	} else if (d->bad_in++ < 5) {
		fprintf(stderr, "npk-camera: frame %ux%u format %u (%zu B) not used\n", w, h, fmt, len);
	}
}

/* The port is readable: take in what came, hand on every whole frame. */
static void on_port(void *userdata, int fd, uint32_t mask)
{
	struct data *d = userdata;
	for (;;) {
		static uint8_t buf[65536];
		ssize_t r = read(fd, buf, sizeof(buf));
		if (r <= 0)
			return;
		size_t at = 0;
		while (at < (size_t)r) {
			uint32_t want = 4;
			if (d->in_len >= 4) {
				uint32_t len = d->in[0] | d->in[1] << 8 | d->in[2] << 16 | (uint32_t)d->in[3] << 24;
				if (len == 0 || len > FRAME_MAX + 16) {
					d->in_len = 0;
					return;
				}
				want = 4 + len;
			}
			size_t take = want - d->in_len;
			if (take > (size_t)r - at)
				take = (size_t)r - at;
			d->in = realloc(d->in, want);
			memcpy(d->in + d->in_len, buf + at, take);
			d->in_len += take;
			at += take;
			if (d->in_len == want && want > 4) {
				if (d->in[4] == CAM_FRAME)
					on_frame(d, d->in + 5, want - 5);
				d->in_len = 0;
			}
		}
	}
}

static void on_timeout(void *userdata, uint64_t expirations)
{
	struct data *d = userdata;
	pw_stream_trigger_process(d->stream);
}

/* Eight vertical colour bars in YUY2 (BT.601), moving one pixel per frame,
 * and a white square crossing the picture: motion and colour both show. */
static void draw(uint8_t *p, uint32_t stride, uint32_t w, uint32_t h, uint32_t frame)
{
	static const uint8_t bars[8][3] = {
		{235, 128, 128}, {210, 16, 146}, {170, 166, 16}, {145, 54, 34},
		{106, 202, 222}, {81, 90, 240}, {41, 240, 110}, {16, 128, 128},
	};
	uint32_t sq = h / 6, sx = (frame * 8) % (w - sq), sy = h / 2 - sq / 2;
	for (uint32_t y = 0; y < h; y++) {
		uint8_t *row = p + y * stride;
		for (uint32_t x = 0; x < w; x += 2) {
			const uint8_t *c = bars[((x + frame) * 8 / w) % 8];
			uint8_t yy = c[0];
			if (x >= sx && x < sx + sq && y >= sy && y < sy + sq)
				yy = 235;
			row[x * 2 + 0] = yy;
			row[x * 2 + 1] = c[1];
			row[x * 2 + 2] = yy;
			row[x * 2 + 3] = c[2];
		}
	}
}

static void on_process(void *userdata)
{
	struct data *d = userdata;
	struct pw_buffer *b = pw_stream_dequeue_buffer(d->stream);
	if (b == NULL)
		return;
	struct spa_buffer *buf = b->buffer;
	uint8_t *p = buf->datas[0].data;
	uint32_t w = d->format.size.width, h = d->format.size.height;
	uint32_t stride = w * 2;
	if (p != NULL && buf->datas[0].maxsize >= stride * h) {
		if (d->port < 0)
			draw(p, stride, w, h, d->frame);
		else if (d->have_frame && d->yuyv_w == w && d->yuyv_h == h)
			memcpy(p, d->yuyv, (size_t)stride * h);
		else
			for (size_t i = 0; i < (size_t)stride * h; i += 2) { p[i] = 16; p[i + 1] = 128; }
		buf->datas[0].chunk->offset = 0;
		buf->datas[0].chunk->size = stride * h;
		buf->datas[0].chunk->stride = stride;
	}
	d->frame++;
	pw_stream_queue_buffer(d->stream, b);
}

static void on_state_changed(void *userdata, enum pw_stream_state old,
			     enum pw_stream_state state, const char *error)
{
	struct data *d = userdata;
	struct pw_loop *l = pw_main_loop_get_loop(d->loop);
	fprintf(stderr, "npk-camera: %s%s%s\n", pw_stream_state_as_string(state),
		error ? ": " : "", error ? error : "");
	if (state == PW_STREAM_STATE_STREAMING) {
		struct timespec start = {0, 1}, interval = {0, 1000000000 / FPS};
		pw_loop_update_timer(l, d->timer, &start, &interval, false);
		uint8_t size[4] = {d->format.size.width & 0xff, d->format.size.width >> 8,
				   d->format.size.height & 0xff, d->format.size.height >> 8};
		d->have_frame = 0;
		port_send(d, CAM_START, size, 4);
	} else if (old == PW_STREAM_STATE_STREAMING) {
		pw_loop_update_timer(l, d->timer, NULL, NULL, false);
		port_send(d, CAM_STOP, NULL, 0);
		fprintf(stderr, "npk-camera: %llu frame(s) from the host, %llu unusable\n",
			(unsigned long long)d->frames_in, (unsigned long long)d->bad_in);
	} else {
		pw_loop_update_timer(l, d->timer, NULL, NULL, false);
	}
}

static void on_param_changed(void *userdata, uint32_t id, const struct spa_pod *param)
{
	struct data *d = userdata;
	if (param == NULL || id != SPA_PARAM_Format)
		return;
	if (spa_format_video_raw_parse(param, &d->format) < 0)
		return;
	fprintf(stderr, "npk-camera: format %ux%u\n", d->format.size.width, d->format.size.height);
	d->yuyv_w = d->format.size.width;
	d->yuyv_h = d->format.size.height;
	free(d->yuyv);
	d->yuyv = calloc((size_t)d->yuyv_w * d->yuyv_h, 2);
	uint8_t pod[1024];
	struct spa_pod_builder b = SPA_POD_BUILDER_INIT(pod, sizeof(pod));
	int32_t stride = (int32_t)d->format.size.width * 2;
	const struct spa_pod *params[1];
	params[0] = spa_pod_builder_add_object(&b,
		SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
		SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
		SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1),
		SPA_PARAM_BUFFERS_size, SPA_POD_Int(stride * (int32_t)d->format.size.height),
		SPA_PARAM_BUFFERS_stride, SPA_POD_Int(stride),
		SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int((1 << SPA_DATA_MemFd) | (1 << SPA_DATA_MemPtr)));
	pw_stream_update_params(d->stream, params, 1);
}

static const struct pw_stream_events stream_events = {
	PW_VERSION_STREAM_EVENTS,
	.state_changed = on_state_changed,
	.param_changed = on_param_changed,
	.process = on_process,
};

int main(int argc, char *argv[])
{
	struct data d = {0};
	pw_init(&argc, &argv);
	d.loop = pw_main_loop_new(NULL);
	if (d.loop == NULL) {
		fprintf(stderr, "npk-camera: no main loop\n");
		return 1;
	}
	d.timer = pw_loop_add_timer(pw_main_loop_get_loop(d.loop), on_timeout, &d);
	d.port = open_port("npk.camera");
	if (d.port >= 0)
		d.port_io = pw_loop_add_io(pw_main_loop_get_loop(d.loop), d.port, SPA_IO_IN, false, on_port, &d);
	fprintf(stderr, "npk-camera: %s\n", d.port >= 0 ? "host camera port open" : "no host camera port, test pattern");
	d.stream = pw_stream_new_simple(pw_main_loop_get_loop(d.loop), "npk-camera",
		pw_properties_new(
			PW_KEY_MEDIA_CLASS, "Video/Source",
			PW_KEY_MEDIA_TYPE, "Video",
			PW_KEY_MEDIA_CATEGORY, "Capture",
			PW_KEY_MEDIA_ROLE, "Camera",
			PW_KEY_NODE_NAME, "npk-camera",
			PW_KEY_NODE_DESCRIPTION, "nopeek camera",
			NULL),
		&stream_events, &d);

	uint8_t pod[1024];
	struct spa_pod_builder b = SPA_POD_BUILDER_INIT(pod, sizeof(pod));
	const struct spa_pod *params[1];
	params[0] = spa_pod_builder_add_object(&b,
		SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
		SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
		SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
		SPA_FORMAT_VIDEO_format, SPA_POD_Id(SPA_VIDEO_FORMAT_YUY2),
		SPA_FORMAT_VIDEO_size, SPA_POD_Rectangle(&SPA_RECTANGLE(WIDTH, HEIGHT)),
		SPA_FORMAT_VIDEO_framerate, SPA_POD_Fraction(&SPA_FRACTION(FPS, 1)));
	int r = pw_stream_connect(d.stream, PW_DIRECTION_OUTPUT, PW_ID_ANY,
		PW_STREAM_FLAG_DRIVER | PW_STREAM_FLAG_MAP_BUFFERS, params, 1);
	if (r < 0) {
		fprintf(stderr, "npk-camera: connect: %s\n", strerror(-r));
		return 1;
	}
	pw_main_loop_run(d.loop);
	pw_stream_destroy(d.stream);
	pw_main_loop_destroy(d.loop);
	pw_deinit();
	return 0;
}
