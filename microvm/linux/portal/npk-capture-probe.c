/*
 * npk-capture-probe: checks the screen capture from inside the guest
 * (`npkcapprobe=<mode>` on the guest cmdline; output on the guest's kmsg).
 *
 *   port       the capture port alone, while no app shares: requests
 *              without a session are refused or dropped, then a session
 *              (the user clicks Share on the host), every kind of bad
 *              buffer refused, a good one taken and filled, the test frame
 *              (`gpu capture test` on the host) read back, STOP.
 *   consume    a PipeWire consumer of portal-npk's stream: the session
 *              through the backend's D-Bus interface (the user clicks
 *              Share), the dmabuf imported, the checksum of every frame
 *              logged, so the host's test frame can be found in it.
 *   malformed  one frame that cannot be one; the host ends the VM.
 *
 * Every check prints PASS or FAIL and the run ends with the counts.
 */
#include <poll.h>
#include <stdlib.h>
#include <time.h>

#include <gio/gio.h>

#include <drm_fourcc.h>

#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>

#include "npkcap.h"

#define CHOICE_TIMEOUT_MS 120000
#define REPLY_TIMEOUT_MS 2000

static int passed, failed;

static void check(int ok, const char *what)
{
	printf("%s %s\n", ok ? "PASS" : "FAIL", what);
	fflush(stdout);
	if (ok)
		passed++;
	else
		failed++;
}

static uint64_t now_ms(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return (uint64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

/* ── port ──────────────────────────────────────────────────────────── */

static uint8_t in[512];
static size_t in_len;
/* Frames the host sent while a reply was awaited. */
static int frames_seen;

/* The next frame from the host within `ms`: its type and payload, or -1. */
static int next_frame(int fd, uint8_t *payload, size_t *n, int ms)
{
	uint64_t end = now_ms() + ms;
	for (;;) {
		if (in_len >= 2) {
			size_t len = get16(in);
			if (len == 0 || len > CAP_MAX_FRAME) {
				printf("bad frame from the host (len %zu)\n", len);
				in_len = 0;
				return -1;
			}
			if (in_len >= 2 + len) {
				int type = in[2];
				*n = len - 1;
				memcpy(payload, in + 3, len - 1);
				memmove(in, in + 2 + len, in_len - 2 - len);
				in_len -= 2 + len;
				return type;
			}
		}
		uint64_t t = now_ms();
		if (t >= end)
			return -1;
		struct pollfd p = { .fd = fd, .events = POLLIN };
		if (poll(&p, 1, (int)(end - t)) <= 0)
			continue;
		ssize_t r = read(fd, in + in_len, sizeof(in) - in_len);
		if (r > 0)
			in_len += r;
	}
}

/* Wait for a frame of `want` within `ms`, skipping others; its payload. */
static int expect(int fd, int want, uint8_t *payload, size_t *n, int ms)
{
	uint64_t end = now_ms() + ms;
	for (;;) {
		uint64_t t = now_ms();
		if (t >= end)
			return 0;
		int type = next_frame(fd, payload, n, (int)(end - t));
		if (type == want)
			return 1;
		if (type < 0)
			return 0;
		printf("  (skipped message %#04x)\n", type);
	}
}

static void send_buffer(int fd, uint8_t slot, uint32_t res, uint32_t offset, uint32_t stride, uint32_t w, uint32_t h)
{
	uint8_t p[20] = {0};
	p[0] = slot;
	put32(p + 4, res);
	put32(p + 8, offset);
	put32(p + 12, stride);
	put16(p + 16, w);
	put16(p + 18, h);
	port_send(fd, CAP_BUFFER, p, sizeof(p));
}

/* A buffer refused with `why`; a frame arriving meanwhile is noted. */
static void expect_refused(int fd, uint8_t slot, int why, const char *what)
{
	uint8_t p[CAP_MAX_FRAME];
	size_t n = 0;
	int type;
	while ((type = next_frame(fd, p, &n, REPLY_TIMEOUT_MS)) == CAP_FRAME)
		frames_seen++;
	char line[160];
	snprintf(line, sizeof(line), "%s (got %#04x %u/%u)", what, type < 0 ? 0 : type,
		 n > 0 ? p[0] : 0, n > 1 ? p[1] : 0);
	check(type == CAP_REFUSED && n >= 2 && p[0] == slot && p[1] == why, line);
}

/* Nothing comes back within a short wait (a dropped request). */
static void expect_silence(int fd, const char *what)
{
	uint8_t p[CAP_MAX_FRAME];
	size_t n = 0;
	check(next_frame(fd, p, &n, 500) < 0, what);
}

static int run_port(void)
{
	int fd = open_port("npk.capture");
	if (fd < 0) {
		printf("no capture port (portal-npk holds it while an app shares): %s\n", strerror(errno));
		return 1;
	}
	struct gbm_device *gbm = gbm_open();
	if (gbm == NULL) {
		printf("no GBM device\n");
		return 1;
	}

	/* No session: nothing is taken. */
	uint32_t res0 = 0;
	struct gbm_bo *pre = bo_create(gbm, 64, 64, &res0);
	send_buffer(fd, 0, pre ? res0 : 1, 0, 256, 64, 64);
	expect_refused(fd, 0, REFUSE_NOT_STARTED, "A3 buffer without a session refused");
	uint8_t slot = 0;
	port_send(fd, CAP_QUEUE, &slot, 1);
	expect_silence(fd, "A3 QUEUE without a session dropped");
	uint8_t two[2] = { 0, 0 };
	port_send(fd, CAP_START, two, 2);
	expect_silence(fd, "A3 START with a short payload dropped");
	if (pre)
		gbm_bo_destroy(pre);

	/* A session: only the host's picker starts it. */
	uint8_t p[CAP_MAX_FRAME];
	size_t n = 0;
	uint8_t want[4];
	put16(want, 1280);
	put16(want + 2, 720);
	port_send(fd, CAP_START, want, 4);
	printf("asked to share; click Share on the host within %d s\n", CHOICE_TIMEOUT_MS / 1000);
	int type = next_frame(fd, p, &n, CHOICE_TIMEOUT_MS);
	if (type != CAP_STARTED || n < 4) {
		check(0, type == CAP_ENDED ? "session (the host ended it: cancelled?)" : "session (no answer)");
		printf("probe: %d passed, %d failed\n", passed, failed);
		return 1;
	}
	uint32_t w = get16(p), h = get16(p + 2);
	char line[160];
	snprintf(line, sizeof(line), "session started at %ux%u (asked at most 1280x720)", w, h);
	check(w <= 1280 && h <= 720 && w && h, line);

	uint32_t res = 0, small_res = 0;
	struct gbm_bo *bo = bo_create(gbm, w, h, &res);
	struct gbm_bo *small = bo_create(gbm, w / 4, h / 4, &small_res);
	if (bo == NULL || small == NULL) {
		check(0, "guest buffers allocated");
		return 1;
	}
	uint32_t stride = gbm_bo_get_stride(bo), offset = gbm_bo_get_offset(bo, 0);
	uint32_t small_stride = gbm_bo_get_stride(small);

	send_buffer(fd, 8, res, offset, stride, w, h);
	expect_refused(fd, 8, REFUSE_SLOT, "A4 slot out of range refused");
	send_buffer(fd, 0, 0x7fffff00, 0, stride, w, h);
	expect_refused(fd, 0, REFUSE_UNKNOWN, "A4 foreign resource refused");
	send_buffer(fd, 0, res, offset, stride, w + 2, h);
	expect_refused(fd, 0, REFUSE_SIZE, "A4 wrong size refused");
	send_buffer(fd, 0, res, offset, w * 4 - 4, w, h);
	expect_refused(fd, 0, REFUSE_PITCH, "A4 pitch below the row refused");
	send_buffer(fd, 0, res, offset + 2, stride, w, h);
	expect_refused(fd, 0, REFUSE_PITCH, "A4 unaligned offset refused");
	send_buffer(fd, 0, small_res, 0, small_stride, w, h);
	expect_refused(fd, 0, REFUSE_TOO_SMALL, "A4 undersized buffer refused");
	send_buffer(fd, 0, res, offset, stride * 4, w, h);
	expect_refused(fd, 0, REFUSE_TOO_SMALL, "A4 pitch past the buffer refused");

	send_buffer(fd, 0, res, offset, stride, w, h);
	type = next_frame(fd, p, &n, REPLY_TIMEOUT_MS);
	check(type == CAP_ACCEPTED && n >= 1 && p[0] == 0, "good buffer accepted");
	send_buffer(fd, 1, res, offset, stride, w, h);
	expect_refused(fd, 1, REFUSE_IN_USE, "A4 the same buffer in a second slot refused");

	/* A frame, verified on the host (its log: "verification ... 0 pixels differ"). */
	int got = frames_seen > 0 || (expect(fd, CAP_FRAME, p, &n, 5000) && n >= 8 && p[0] == 0);
	check(got, "first frame arrived in the good buffer");

	/* The test frame: `gpu capture test` on the host. */
	printf("run `gpu capture test` on the host within %d s\n", CHOICE_TIMEOUT_MS / 1000);
	uint64_t end = now_ms() + CHOICE_TIMEOUT_MS;
	int test = 0;
	while (!test && now_ms() < end) {
		slot = 0;
		port_send(fd, CAP_QUEUE, &slot, 1);
		if (!expect(fd, CAP_FRAME, p, &n, (int)(end - now_ms())))
			break;
		test = n >= 2 && (p[1] & FRAME_TEST);
	}
	if (test) {
		int ok;
		uint32_t crc = bo_crc32(bo, w, h, &ok);
		printf("test frame %ux%u at the port, crc32 %08x (compare the host's)\n", w, h, crc);
		check(ok, "test frame read back");
	} else {
		check(0, "test frame arrived");
	}

	port_send(fd, CAP_STOP, NULL, 0);
	printf("stopped; the host's `gpu capture` should show 0 buffers bound shortly\n");
	gbm_bo_destroy(small);
	gbm_bo_destroy(bo);
	close(fd);
	printf("probe: %d passed, %d failed\n", passed, failed);
	return failed != 0;
}

/* ── malformed ─────────────────────────────────────────────────────── */

static int run_malformed(void)
{
	int fd = open_port("npk.capture");
	if (fd < 0) {
		printf("no capture port: %s\n", strerror(errno));
		return 1;
	}
	uint8_t bad[3] = { 0xff, 0x7f, CAP_START };
	printf("sending a frame of length 32767; the host ends the VM now\n");
	fflush(stdout);
	if (write(fd, bad, sizeof(bad)) != sizeof(bad))
		printf("write: %s\n", strerror(errno));
	sleep(5);
	printf("FAIL the VM is still running\n");
	return 1;
}

/* ── consume ───────────────────────────────────────────────────────── */

struct consumer {
	struct pw_main_loop *loop;
	struct pw_stream *stream;
	struct spa_hook listener;
	struct gbm_device *gbm;
	struct spa_video_info_raw format;
	uint32_t frames, logged;
	uint64_t last_log;
};

static void on_param_changed(void *data, uint32_t id, const struct spa_pod *param)
{
	struct consumer *c = data;
	if (id != SPA_PARAM_Format || param == NULL)
		return;
	if (spa_format_video_raw_parse(param, &c->format) < 0)
		return;
	printf("consumer: format %ux%u modifier %#llx\n", c->format.size.width, c->format.size.height,
	       (unsigned long long)c->format.modifier);
	uint8_t buf[256];
	struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof(buf));
	const struct spa_pod *params[1] = { spa_pod_builder_add_object(&b,
		SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
		SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int(1 << SPA_DATA_DmaBuf)) };
	pw_stream_update_params(c->stream, params, 1);
}

static void on_process(void *data)
{
	struct consumer *c = data;
	struct pw_buffer *b = pw_stream_dequeue_buffer(c->stream);
	if (b == NULL)
		return;
	struct spa_data *d = &b->buffer->datas[0];
	c->frames++;
	uint64_t t = now_ms();
	if (d->type == SPA_DATA_DmaBuf && d->fd >= 0 && (t - c->last_log >= 200 || c->logged < 3)) {
		struct gbm_import_fd_modifier_data imp = {
			.width = c->format.size.width, .height = c->format.size.height,
			.format = GBM_FORMAT_XRGB8888, .num_fds = 1,
			.fds = { (int)d->fd }, .strides = { (int)d->chunk->stride }, .offsets = { (int)d->chunk->offset },
			.modifier = DRM_FORMAT_MOD_LINEAR,
		};
		struct gbm_bo *bo = gbm_bo_import(c->gbm, GBM_BO_IMPORT_FD_MODIFIER, &imp, GBM_BO_USE_LINEAR);
		int ok = 0;
		uint32_t crc = bo ? bo_crc32(bo, imp.width, imp.height, &ok) : 0;
		if (bo)
			gbm_bo_destroy(bo);
		struct spa_meta_header *h = spa_buffer_find_meta_data(b->buffer, SPA_META_Header, sizeof(*h));
		if (ok)
			printf("consumer: frame %u (seq %llu) %ux%u crc32 %08x\n", c->frames,
			       h ? (unsigned long long)h->seq : 0ULL, imp.width, imp.height, crc);
		else
			printf("consumer: frame %u: dmabuf not importable\n", c->frames);
		fflush(stdout);
		c->logged++;
		c->last_log = t;
	} else if (d->type != SPA_DATA_DmaBuf && c->logged++ < 3) {
		printf("consumer: frame %u is not a dmabuf (type %u)\n", c->frames, d->type);
	}
	pw_stream_queue_buffer(c->stream, b);
}

static void on_state_changed(void *data, enum pw_stream_state old, enum pw_stream_state state, const char *error)
{
	printf("consumer: stream %s%s%s\n", pw_stream_state_as_string(state), error ? ": " : "", error ? error : "");
	fflush(stdout);
}

static const struct pw_stream_events consumer_events = {
	PW_VERSION_STREAM_EVENTS,
	.state_changed = on_state_changed,
	.param_changed = on_param_changed,
	.process = on_process,
};

static void stop_after(void *data, uint64_t expirations)
{
	pw_main_loop_quit(data);
}

/* Call the backend directly; the reply of a method that waits for the user. */
static GVariant *backend_call(GDBusConnection *bus, const char *method, GVariant *args, int ms)
{
	GError *err = NULL;
	GVariant *r = g_dbus_connection_call_sync(bus, "org.freedesktop.impl.portal.desktop.npk",
		"/org/freedesktop/portal/desktop", "org.freedesktop.impl.portal.ScreenCast", method, args,
		G_VARIANT_TYPE("(ua{sv})"), G_DBUS_CALL_FLAGS_NONE, ms, NULL, &err);
	if (r == NULL) {
		printf("%s: %s\n", method, err->message);
		g_error_free(err);
	}
	return r;
}

static int run_consume(void)
{
	GError *err = NULL;
	GDBusConnection *bus = g_bus_get_sync(G_BUS_TYPE_SESSION, NULL, &err);
	if (bus == NULL) {
		printf("no session bus: %s\n", err->message);
		return 1;
	}
	const char *session = "/org/freedesktop/portal/desktop/session/npkprobe/s1";
	const char *app = "npk-capture-probe";
	GVariant *r = backend_call(bus, "CreateSession",
		g_variant_new("(oosa{sv})", "/org/freedesktop/portal/desktop/request/npkprobe/r1", session, app, NULL),
		REPLY_TIMEOUT_MS * 5);
	check(r != NULL, "CreateSession");
	if (r)
		g_variant_unref(r);
	r = backend_call(bus, "SelectSources",
		g_variant_new("(oosa{sv})", "/org/freedesktop/portal/desktop/request/npkprobe/r2", session, app, NULL),
		REPLY_TIMEOUT_MS);
	if (r)
		g_variant_unref(r);
	printf("asked to share; click Share on the host within %d s\n", CHOICE_TIMEOUT_MS / 1000);
	fflush(stdout);
	r = backend_call(bus, "Start",
		g_variant_new("(oossa{sv})", "/org/freedesktop/portal/desktop/request/npkprobe/r3", session, app, "", NULL),
		CHOICE_TIMEOUT_MS + 10000);
	uint32_t response = 2, node = 0;
	if (r) {
		GVariant *results = NULL;
		g_variant_get(r, "(u@a{sv})", &response, &results);
		GVariant *streams = g_variant_lookup_value(results, "streams", G_VARIANT_TYPE("a(ua{sv})"));
		if (streams && g_variant_n_children(streams) > 0)
			g_variant_get_child(streams, 0, "(u@a{sv})", &node, NULL);
		if (streams)
			g_variant_unref(streams);
		g_variant_unref(results);
		g_variant_unref(r);
	}
	char line[96];
	snprintf(line, sizeof(line), "Start answered %u with node %u", response, node);
	check(response == 0 && node != 0, line);
	if (response != 0 || node == 0)
		return 1;

	struct consumer c = {0};
	c.gbm = gbm_open();
	if (c.gbm == NULL) {
		check(0, "GBM device");
		return 1;
	}
	pw_init(NULL, NULL);
	c.loop = pw_main_loop_new(NULL);
	c.stream = pw_stream_new_simple(pw_main_loop_get_loop(c.loop), "npk-capture-probe",
		pw_properties_new(PW_KEY_MEDIA_TYPE, "Video", PW_KEY_MEDIA_CATEGORY, "Capture",
				  PW_KEY_MEDIA_ROLE, "Screen", NULL),
		&consumer_events, &c);
	uint8_t buf[1024];
	struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof(buf));
	struct spa_pod_frame f;
	spa_pod_builder_push_object(&b, &f, SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat);
	spa_pod_builder_add(&b,
		SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
		SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
		SPA_FORMAT_VIDEO_format, SPA_POD_Id(SPA_VIDEO_FORMAT_BGRx),
		SPA_FORMAT_VIDEO_size, SPA_POD_CHOICE_RANGE_Rectangle(&SPA_RECTANGLE(1280, 720), &SPA_RECTANGLE(1, 1),
								      &SPA_RECTANGLE(8192, 8192)),
		SPA_FORMAT_VIDEO_framerate, SPA_POD_CHOICE_RANGE_Fraction(&SPA_FRACTION(0, 1), &SPA_FRACTION(0, 1),
									  &SPA_FRACTION(1000, 1)),
		0);
	spa_pod_builder_prop(&b, SPA_FORMAT_VIDEO_modifier, SPA_POD_PROP_FLAG_MANDATORY);
	spa_pod_builder_long(&b, DRM_FORMAT_MOD_LINEAR);
	const struct spa_pod *params[1] = { spa_pod_builder_pop(&b, &f) };
	pw_stream_connect(c.stream, PW_DIRECTION_INPUT, node, PW_STREAM_FLAG_AUTOCONNECT, params, 1);
	printf("consuming for %d s; run `gpu capture test` on the host meanwhile\n", CHOICE_TIMEOUT_MS / 1000);
	fflush(stdout);
	struct spa_source *timer = pw_loop_add_timer(pw_main_loop_get_loop(c.loop), stop_after, c.loop);
	struct timespec to = { .tv_sec = CHOICE_TIMEOUT_MS / 1000 };
	pw_loop_update_timer(pw_main_loop_get_loop(c.loop), timer, &to, NULL, false);
	pw_main_loop_run(c.loop);
	check(c.frames > 0, "frames reached the PipeWire consumer");
	pw_stream_destroy(c.stream);
	pw_main_loop_destroy(c.loop);

	GVariant *closed = g_dbus_connection_call_sync(bus, "org.freedesktop.impl.portal.desktop.npk", session,
		"org.freedesktop.impl.portal.Session", "Close", NULL, NULL, G_DBUS_CALL_FLAGS_NONE, REPLY_TIMEOUT_MS,
		NULL, NULL);
	check(closed != NULL, "session closed");
	if (closed)
		g_variant_unref(closed);
	printf("probe: %d passed, %d failed (%u frames consumed)\n", passed, failed, c.frames);
	return failed != 0;
}

int main(int argc, char **argv)
{
	setvbuf(stdout, NULL, _IOLBF, 0);
	const char *mode = argc > 1 ? argv[1] : "";
	if (strcmp(mode, "port") == 0)
		return run_port();
	if (strcmp(mode, "consume") == 0)
		return run_consume();
	if (strcmp(mode, "malformed") == 0)
		return run_malformed();
	printf("usage: npk-capture-probe port | consume | malformed\n");
	return 2;
}
