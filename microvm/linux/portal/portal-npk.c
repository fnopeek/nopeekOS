/*
 * portal-npk: nopeekOS's xdg-desktop-portal backend in the guest.
 *
 * Access: xdg-desktop-portal offers the Camera portal only when a backend
 * implements org.freedesktop.impl.portal.Access. This one does, and grants:
 * the browser asks the user before a page gets the camera, and the host
 * shows while the camera runs.
 *
 * ScreenCast: the whole nopeekOS screen. Start asks the host over the
 * virtio-console port "npk.capture" (kernel/src/shade/capture.rs); the host
 * shows its own picker, and only the user's choice there starts a session.
 * The stream is a PipeWire video source whose buffers are GBM buffers on
 * the guest's GPU, handed to consumers as dmabuf; the host names each one
 * by its virtio-gpu resource, its GPU scales the screen into them, and the
 * port says which one holds a new frame. No pixel crosses the port. The
 * cursor is not in the frames (cursor mode Hidden).
 *
 * D-Bus name org.freedesktop.impl.portal.desktop.npk, activated by the
 * session bus; npk.portal and portals.conf tell xdg-desktop-portal to use it.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <gio/gio.h>
#include <glib-unix.h>

#include <drm_fourcc.h>

#include <pipewire/pipewire.h>
#include <spa/buffer/meta.h>
#include <spa/param/video/format-utils.h>

#include "npkcap.h"

/* Buffers per stream (the host takes up to 8). */
#define MAX_SLOTS 8
/* The largest stream asked for; a call needs no more, and the host scales. */
#define MAX_W 1920
#define MAX_H 1080

#define SOURCE_MONITOR 1
#define CURSOR_HIDDEN 1

static const char introspection[] =
	"<node>"
	"  <interface name='org.freedesktop.impl.portal.Access'>"
	"    <method name='AccessDialog'>"
	"      <arg type='o' name='handle' direction='in'/>"
	"      <arg type='s' name='app_id' direction='in'/>"
	"      <arg type='s' name='parent_window' direction='in'/>"
	"      <arg type='s' name='title' direction='in'/>"
	"      <arg type='s' name='subtitle' direction='in'/>"
	"      <arg type='s' name='body' direction='in'/>"
	"      <arg type='a{sv}' name='options' direction='in'/>"
	"      <arg type='u' name='response' direction='out'/>"
	"      <arg type='a{sv}' name='results' direction='out'/>"
	"    </method>"
	"  </interface>"
	"  <interface name='org.freedesktop.impl.portal.ScreenCast'>"
	"    <method name='CreateSession'>"
	"      <arg type='o' name='handle' direction='in'/>"
	"      <arg type='o' name='session_handle' direction='in'/>"
	"      <arg type='s' name='app_id' direction='in'/>"
	"      <arg type='a{sv}' name='options' direction='in'/>"
	"      <arg type='u' name='response' direction='out'/>"
	"      <arg type='a{sv}' name='results' direction='out'/>"
	"    </method>"
	"    <method name='SelectSources'>"
	"      <arg type='o' name='handle' direction='in'/>"
	"      <arg type='o' name='session_handle' direction='in'/>"
	"      <arg type='s' name='app_id' direction='in'/>"
	"      <arg type='a{sv}' name='options' direction='in'/>"
	"      <arg type='u' name='response' direction='out'/>"
	"      <arg type='a{sv}' name='results' direction='out'/>"
	"    </method>"
	"    <method name='Start'>"
	"      <arg type='o' name='handle' direction='in'/>"
	"      <arg type='o' name='session_handle' direction='in'/>"
	"      <arg type='s' name='app_id' direction='in'/>"
	"      <arg type='s' name='parent_window' direction='in'/>"
	"      <arg type='a{sv}' name='options' direction='in'/>"
	"      <arg type='u' name='response' direction='out'/>"
	"      <arg type='a{sv}' name='results' direction='out'/>"
	"    </method>"
	"    <property name='AvailableSourceTypes' type='u' access='read'/>"
	"    <property name='AvailableCursorModes' type='u' access='read'/>"
	"    <property name='version' type='u' access='read'/>"
	"  </interface>"
	"  <interface name='org.freedesktop.impl.portal.Session'>"
	"    <method name='Close'/>"
	"    <signal name='Closed'/>"
	"    <property name='version' type='u' access='read'/>"
	"  </interface>"
	"</node>";

struct slot {
	struct pw_buffer *pwb;
	struct gbm_bo *bo;
	int fd;
	uint32_t res;
	int accepted;
	/* Dequeued from PipeWire: the host's to fill, or filling. */
	int held;
};

static struct {
	GDBusConnection *conn;
	GDBusNodeInfo *info;
	/* The one session (the host shares one screen with one guest app). */
	char *session_path;
	guint session_reg;
	GDBusMethodInvocation *start_inv;
	/* The capture port while a session asks or runs. */
	int port;
	guint port_watch;
	uint8_t in[512];
	size_t in_len;
	/* `on_port` is running: it removes its own watch. */
	int in_port;
	/* The stream. */
	uint32_t w, h;
	struct pw_stream *stream;
	struct spa_hook stream_listener;
	struct slot slots[MAX_SLOTS];
	uint64_t frames;
} cast = { .port = -1 };

static struct pw_loop *pwloop;
static struct pw_context *pwctx;
static struct pw_core *pwcore;
static struct gbm_device *gbm;

static void cast_end(int tell_host, int closed_signal);

static void cast_send(uint8_t type, const uint8_t *payload, uint16_t n)
{
	if (cast.port >= 0 && !port_send(cast.port, type, payload, n))
		fprintf(stderr, "portal-npk: port write: %s\n", strerror(errno));
}

/* ── GBM buffers ───────────────────────────────────────────────────── */

static int gpu_open(void)
{
	if (gbm == NULL)
		gbm = gbm_open();
	if (gbm == NULL) {
		fprintf(stderr, "portal-npk: no GBM device on a render node\n");
		return -1;
	}
	return 0;
}

/* A buffer of the stream's size, exported as dmabuf. */
static int slot_alloc(struct slot *s)
{
	s->bo = bo_create(gbm, cast.w, cast.h, &s->res);
	if (s->bo == NULL) {
		fprintf(stderr, "portal-npk: buffer %ux%u: %s\n", cast.w, cast.h, strerror(errno));
		return -1;
	}
	s->fd = gbm_bo_get_fd(s->bo);
	if (s->fd < 0) {
		gbm_bo_destroy(s->bo);
		s->bo = NULL;
		return -1;
	}
	return 0;
}

static void slot_free(struct slot *s)
{
	if (s->fd >= 0)
		close(s->fd);
	if (s->bo != NULL)
		gbm_bo_destroy(s->bo);
	memset(s, 0, sizeof(*s));
	s->fd = -1;
}

/* Log the test frame's checksum, as the host logs it. */
static void log_test_frame(struct slot *s, uint32_t seq)
{
	int ok;
	uint32_t crc = bo_crc32(s->bo, cast.w, cast.h, &ok);
	if (ok)
		fprintf(stderr, "portal-npk: test frame %ux%u handed to PipeWire, crc32 %08x\n", cast.w, cast.h, crc);
	else
		fprintf(stderr, "portal-npk: test frame %u: cannot map\n", seq);
}

/* ── The stream ────────────────────────────────────────────────────── */

static int slot_of(struct pw_buffer *b)
{
	return (int)(uintptr_t)b->user_data - 1;
}

/* Take every buffer the consumer let go of and hand it to the host. */
static void take_free_buffers(void)
{
	struct pw_buffer *b;
	while (cast.stream != NULL && (b = pw_stream_dequeue_buffer(cast.stream)) != NULL) {
		int i = slot_of(b);
		if (i < 0 || i >= MAX_SLOTS)
			continue;
		cast.slots[i].held = 1;
		if (cast.slots[i].accepted) {
			uint8_t p = i;
			cast_send(CAP_QUEUE, &p, 1);
		}
	}
}

static void on_process(void *data)
{
	take_free_buffers();
}

static void reply_start(uint32_t response)
{
	if (cast.start_inv == NULL)
		return;
	GVariantBuilder results;
	g_variant_builder_init(&results, G_VARIANT_TYPE_VARDICT);
	if (response == 0) {
		GVariantBuilder streams, props;
		g_variant_builder_init(&streams, G_VARIANT_TYPE("a(ua{sv})"));
		g_variant_builder_init(&props, G_VARIANT_TYPE_VARDICT);
		g_variant_builder_add(&props, "{sv}", "source_type", g_variant_new_uint32(SOURCE_MONITOR));
		g_variant_builder_add(&props, "{sv}", "size", g_variant_new("(ii)", (int)cast.w, (int)cast.h));
		g_variant_builder_add(&streams, "(ua{sv})", pw_stream_get_node_id(cast.stream), &props);
		g_variant_builder_add(&results, "{sv}", "streams", g_variant_builder_end(&streams));
	}
	g_dbus_method_invocation_return_value(cast.start_inv, g_variant_new("(ua{sv})", response, &results));
	cast.start_inv = NULL;
}

/* A stream that failed after Start ends the session, outside the stream's
 * own callback since that destroys the stream. */
static gboolean end_after_error(gpointer user)
{
	if (cast.session_path != NULL)
		cast_end(1, 1);
	return G_SOURCE_REMOVE;
}

static void on_state_changed(void *data, enum pw_stream_state old, enum pw_stream_state state, const char *error)
{
	fprintf(stderr, "portal-npk: stream %s%s%s\n", pw_stream_state_as_string(state),
		error ? ": " : "", error ? error : "");
	switch (state) {
	case PW_STREAM_STATE_PAUSED:
		reply_start(0);
		break;
	case PW_STREAM_STATE_STREAMING:
		take_free_buffers();
		pw_stream_trigger_process(cast.stream);
		break;
	case PW_STREAM_STATE_ERROR:
		if (cast.start_inv != NULL)
			reply_start(2);
		else
			g_idle_add(end_after_error, NULL);
		break;
	default:
		break;
	}
}

static void on_param_changed(void *data, uint32_t id, const struct spa_pod *param)
{
	if (id != SPA_PARAM_Format || param == NULL)
		return;
	struct spa_video_info_raw fmt;
	if (spa_format_video_raw_parse(param, &fmt) < 0)
		return;
	fprintf(stderr, "portal-npk: format %ux%u, modifier %#llx\n", fmt.size.width, fmt.size.height,
		(unsigned long long)fmt.modifier);
	uint8_t buf[1024];
	struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof(buf));
	const struct spa_pod *params[2];
	params[0] = spa_pod_builder_add_object(&b, SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
		SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, MAX_SLOTS),
		SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1),
		SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int(1 << SPA_DATA_DmaBuf));
	params[1] = spa_pod_builder_add_object(&b, SPA_TYPE_OBJECT_ParamMeta, SPA_PARAM_Meta,
		SPA_PARAM_META_type, SPA_POD_Id(SPA_META_Header),
		SPA_PARAM_META_size, SPA_POD_Int(sizeof(struct spa_meta_header)));
	pw_stream_update_params(cast.stream, params, 2);
}

static void on_add_buffer(void *data, struct pw_buffer *b)
{
	struct spa_data *d = b->buffer->datas;
	int i = 0;
	while (i < MAX_SLOTS && cast.slots[i].pwb != NULL)
		i++;
	if (i == MAX_SLOTS || !(d[0].type & (1 << SPA_DATA_DmaBuf)) || slot_alloc(&cast.slots[i]) < 0) {
		fprintf(stderr, "portal-npk: buffer not allocated\n");
		return;
	}
	struct slot *s = &cast.slots[i];
	s->pwb = b;
	b->user_data = (void *)(uintptr_t)(i + 1);
	uint32_t stride = gbm_bo_get_stride(s->bo), offset = gbm_bo_get_offset(s->bo, 0);
	d[0].type = SPA_DATA_DmaBuf;
	d[0].flags = SPA_DATA_FLAG_READWRITE;
	d[0].fd = s->fd;
	d[0].mapoffset = 0;
	d[0].maxsize = offset + stride * cast.h;
	d[0].data = NULL;
	d[0].chunk->offset = offset;
	d[0].chunk->stride = stride;
	d[0].chunk->size = stride * cast.h;

	uint8_t p[20] = {0};
	p[0] = i;
	put32(p + 4, s->res);
	put32(p + 8, offset);
	put32(p + 12, stride);
	put16(p + 16, cast.w);
	put16(p + 18, cast.h);
	cast_send(CAP_BUFFER, p, sizeof(p));
}

static void on_remove_buffer(void *data, struct pw_buffer *b)
{
	int i = slot_of(b);
	if (i >= 0 && i < MAX_SLOTS)
		slot_free(&cast.slots[i]);
}

static const struct pw_stream_events stream_events = {
	PW_VERSION_STREAM_EVENTS,
	.state_changed = on_state_changed,
	.param_changed = on_param_changed,
	.add_buffer = on_add_buffer,
	.remove_buffer = on_remove_buffer,
	.process = on_process,
};

static int stream_start(void)
{
	if (gpu_open() < 0)
		return -1;
	if (pwcore == NULL)
		pwcore = pw_context_connect(pwctx, NULL, 0);
	if (pwcore == NULL) {
		fprintf(stderr, "portal-npk: no PipeWire\n");
		return -1;
	}
	for (int i = 0; i < MAX_SLOTS; i++) {
		memset(&cast.slots[i], 0, sizeof(cast.slots[i]));
		cast.slots[i].fd = -1;
	}
	cast.stream = pw_stream_new(pwcore, "npk-screen",
		pw_properties_new(PW_KEY_MEDIA_CLASS, "Video/Source", PW_KEY_NODE_NAME, "npk-screen",
				  PW_KEY_NODE_DESCRIPTION, "nopeekOS screen", NULL));
	if (cast.stream == NULL)
		return -1;
	pw_stream_add_listener(cast.stream, &cast.stream_listener, &stream_events, NULL);

	uint8_t buf[1024];
	struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof(buf));
	struct spa_pod_frame f;
	spa_pod_builder_push_object(&b, &f, SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat);
	spa_pod_builder_add(&b,
		SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
		SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
		SPA_FORMAT_VIDEO_format, SPA_POD_Id(SPA_VIDEO_FORMAT_BGRx),
		SPA_FORMAT_VIDEO_size, SPA_POD_Rectangle(&SPA_RECTANGLE(cast.w, cast.h)),
		SPA_FORMAT_VIDEO_framerate, SPA_POD_Fraction(&SPA_FRACTION(0, 1)),
		SPA_FORMAT_VIDEO_maxFramerate,
		SPA_POD_CHOICE_RANGE_Fraction(&SPA_FRACTION(60, 1), &SPA_FRACTION(1, 1), &SPA_FRACTION(60, 1)),
		0);
	/* The buffers are linear; a consumer that can only import with the
	 * implicit modifier (Firefox's WebRTC) gets the same linear buffer,
	 * whose layout the driver reads from the buffer itself. */
	struct spa_pod_frame c;
	spa_pod_builder_prop(&b, SPA_FORMAT_VIDEO_modifier, SPA_POD_PROP_FLAG_MANDATORY);
	spa_pod_builder_push_choice(&b, &c, SPA_CHOICE_Enum, 0);
	spa_pod_builder_long(&b, DRM_FORMAT_MOD_LINEAR);
	spa_pod_builder_long(&b, DRM_FORMAT_MOD_LINEAR);
	spa_pod_builder_long(&b, DRM_FORMAT_MOD_INVALID);
	spa_pod_builder_pop(&b, &c);
	const struct spa_pod *params[1] = { spa_pod_builder_pop(&b, &f) };

	int r = pw_stream_connect(cast.stream, PW_DIRECTION_OUTPUT, PW_ID_ANY,
		PW_STREAM_FLAG_DRIVER | PW_STREAM_FLAG_ALLOC_BUFFERS, params, 1);
	if (r < 0) {
		fprintf(stderr, "portal-npk: connect: %s\n", strerror(-r));
		return -1;
	}
	return 0;
}

/* A new frame in `slot`: to the consumer. */
static void frame_in(uint32_t slot, uint32_t flags, uint32_t seq)
{
	if (slot >= MAX_SLOTS || cast.stream == NULL)
		return;
	struct slot *s = &cast.slots[slot];
	if (s->pwb == NULL || !s->held) {
		fprintf(stderr, "portal-npk: frame %u in buffer %u we do not hold\n", seq, slot);
		return;
	}
	if (flags & FRAME_TEST)
		log_test_frame(s, seq);
	struct spa_buffer *buf = s->pwb->buffer;
	uint32_t stride = gbm_bo_get_stride(s->bo);
	buf->datas[0].chunk->offset = gbm_bo_get_offset(s->bo, 0);
	buf->datas[0].chunk->stride = stride;
	buf->datas[0].chunk->size = stride * cast.h;
	buf->datas[0].chunk->flags = SPA_CHUNK_FLAG_NONE;
	struct spa_meta_header *h = spa_buffer_find_meta_data(buf, SPA_META_Header, sizeof(*h));
	if (h != NULL) {
		struct timespec ts;
		clock_gettime(CLOCK_MONOTONIC, &ts);
		h->pts = SPA_TIMESPEC_TO_NSEC(&ts);
		h->flags = 0;
		h->seq = seq;
		h->dts_offset = 0;
	}
	s->held = 0;
	pw_stream_queue_buffer(cast.stream, s->pwb);
	pw_stream_trigger_process(cast.stream);
	cast.frames++;
	if (cast.frames == 1 || cast.frames % 900 == 0)
		fprintf(stderr, "portal-npk: %llu frame(s) from the host\n", (unsigned long long)cast.frames);
}

/* ── The host's port ───────────────────────────────────────────────── */

static void port_frame(uint8_t type, const uint8_t *p, size_t n)
{
	switch (type) {
	case CAP_STARTED:
		if (n < 4)
			return;
		cast.w = get16(p);
		cast.h = get16(p + 2);
		fprintf(stderr, "portal-npk: the user shares the screen, %ux%u\n", cast.w, cast.h);
		if (stream_start() < 0) {
			reply_start(2);
			cast_end(1, 1);
		}
		break;
	case CAP_ENDED: {
		uint32_t why = n ? p[0] : 0;
		fprintf(stderr, "portal-npk: the host ended the session (%u)\n", why);
		if (cast.start_inv != NULL) {
			reply_start(why == END_CANCELLED ? 1 : 2);
			cast_end(0, 0);
		} else {
			cast_end(0, 1);
		}
		break;
	}
	case CAP_ACCEPTED:
		if (n >= 1 && p[0] < MAX_SLOTS) {
			struct slot *s = &cast.slots[p[0]];
			s->accepted = 1;
			if (s->held) {
				uint8_t i = p[0];
				cast_send(CAP_QUEUE, &i, 1);
			}
		}
		break;
	case CAP_REFUSED:
		if (n >= 2)
			fprintf(stderr, "portal-npk: the host refused buffer %u (%u)\n", p[0], p[1]);
		break;
	case CAP_FRAME:
		if (n >= 8)
			frame_in(p[0], p[1], get32(p + 4));
		break;
	default:
		break;
	}
}

static gboolean on_port(gint fd, GIOCondition cond, gpointer user)
{
	for (;;) {
		ssize_t r = read(fd, cast.in + cast.in_len, sizeof(cast.in) - cast.in_len);
		if (r <= 0)
			break;
		cast.in_len += r;
		size_t at = 0;
		while (cast.in_len - at >= 2) {
			size_t len = get16(cast.in + at);
			if (len == 0 || len > CAP_MAX_FRAME) {
				fprintf(stderr, "portal-npk: bad frame from the host\n");
				cast.in_len = at = 0;
				break;
			}
			if (cast.in_len - at < 2 + len)
				break;
			cast.in_port = 1;
			port_frame(cast.in[at + 2], cast.in + at + 3, len - 1);
			cast.in_port = 0;
			if (cast.port < 0)
				return G_SOURCE_REMOVE;
			at += 2 + len;
		}
		memmove(cast.in, cast.in + at, cast.in_len - at);
		cast.in_len -= at;
	}
	return G_SOURCE_CONTINUE;
}

/* ── Session ───────────────────────────────────────────────────────── */

/* The session is over: the stream, the buffers, the port, the session
 * object go. */
static void cast_end(int tell_host, int closed_signal)
{
	if (cast.stream != NULL) {
		pw_stream_destroy(cast.stream);
		cast.stream = NULL;
	}
	for (int i = 0; i < MAX_SLOTS; i++)
		if (cast.slots[i].bo != NULL)
			slot_free(&cast.slots[i]);
	if (tell_host)
		cast_send(CAP_STOP, NULL, 0);
	if (cast.port_watch && !cast.in_port)
		g_source_remove(cast.port_watch);
	cast.port_watch = 0;
	if (cast.port >= 0) {
		close(cast.port);
		cast.port = -1;
	}
	cast.in_len = 0;
	if (cast.session_path != NULL) {
		if (closed_signal)
			g_dbus_connection_emit_signal(cast.conn, NULL, cast.session_path,
				"org.freedesktop.impl.portal.Session", "Closed", NULL, NULL);
		g_dbus_connection_unregister_object(cast.conn, cast.session_reg);
		g_free(cast.session_path);
		cast.session_path = NULL;
	}
	if (cast.frames)
		fprintf(stderr, "portal-npk: session over after %llu frame(s)\n", (unsigned long long)cast.frames);
	cast.frames = 0;
}

static void on_session_call(GDBusConnection *conn, const gchar *sender, const gchar *path,
			    const gchar *iface, const gchar *method, GVariant *params,
			    GDBusMethodInvocation *inv, gpointer user)
{
	if (g_strcmp0(method, "Close") == 0) {
		fprintf(stderr, "portal-npk: the app closed the session\n");
		g_dbus_method_invocation_return_value(inv, NULL);
		if (cast.start_inv != NULL)
			reply_start(1);
		cast_end(1, 0);
		return;
	}
	g_dbus_method_invocation_return_error(inv, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_METHOD, "%s", method);
}

static GVariant *on_get_property(GDBusConnection *conn, const gchar *sender, const gchar *path,
				 const gchar *iface, const gchar *prop, GError **err, gpointer user)
{
	if (g_strcmp0(prop, "AvailableSourceTypes") == 0)
		return g_variant_new_uint32(SOURCE_MONITOR);
	if (g_strcmp0(prop, "AvailableCursorModes") == 0)
		return g_variant_new_uint32(CURSOR_HIDDEN);
	if (g_strcmp0(prop, "version") == 0)
		return g_variant_new_uint32(g_strcmp0(iface, "org.freedesktop.impl.portal.ScreenCast") == 0 ? 5 : 1);
	return NULL;
}

static const GDBusInterfaceVTable session_vtable = { on_session_call, on_get_property, NULL, { 0 } };

static GDBusInterfaceInfo *iface(const char *name)
{
	return g_dbus_node_info_lookup_interface(cast.info, name);
}

static void empty_reply(GDBusMethodInvocation *inv, uint32_t response)
{
	GVariantBuilder results;
	g_variant_builder_init(&results, G_VARIANT_TYPE_VARDICT);
	g_dbus_method_invocation_return_value(inv, g_variant_new("(ua{sv})", response, &results));
}

static void screencast_call(const gchar *method, GVariant *params, GDBusMethodInvocation *inv)
{
	const gchar *session = NULL, *app_id = NULL;
	g_variant_get_child(params, 1, "&o", &session);
	g_variant_get_child(params, 2, "&s", &app_id);
	if (g_strcmp0(method, "CreateSession") == 0) {
		if (cast.session_path != NULL) {
			if (cast.start_inv != NULL)
				reply_start(2);
			cast_end(1, 1);
		}
		GError *err = NULL;
		guint reg = g_dbus_connection_register_object(cast.conn, session,
			iface("org.freedesktop.impl.portal.Session"), &session_vtable, NULL, NULL, &err);
		if (reg == 0) {
			fprintf(stderr, "portal-npk: session: %s\n", err->message);
			g_error_free(err);
			empty_reply(inv, 2);
			return;
		}
		cast.session_path = g_strdup(session);
		cast.session_reg = reg;
		fprintf(stderr, "portal-npk: screen-cast session for '%s'\n", app_id);
		empty_reply(inv, 0);
		return;
	}
	if (cast.session_path == NULL || g_strcmp0(session, cast.session_path) != 0) {
		empty_reply(inv, 2);
		return;
	}
	if (g_strcmp0(method, "SelectSources") == 0) {
		empty_reply(inv, 0);
		return;
	}
	/* Start: the host asks the user; the answer comes on the port. */
	if (cast.start_inv != NULL || cast.port >= 0) {
		empty_reply(inv, 2);
		return;
	}
	cast.port = open_port("npk.capture");
	if (cast.port < 0) {
		fprintf(stderr, "portal-npk: no capture port: %s\n", strerror(errno));
		empty_reply(inv, 2);
		return;
	}
	cast.port_watch = g_unix_fd_add(cast.port, G_IO_IN, on_port, NULL);
	cast.start_inv = inv;
	uint8_t p[4];
	put16(p, MAX_W);
	put16(p + 2, MAX_H);
	cast_send(CAP_START, p, sizeof(p));
}

static void on_call(GDBusConnection *conn, const gchar *sender, const gchar *path,
		    const gchar *iface_name, const gchar *method, GVariant *params,
		    GDBusMethodInvocation *inv, gpointer user)
{
	if (g_strcmp0(iface_name, "org.freedesktop.impl.portal.ScreenCast") == 0) {
		screencast_call(method, params, inv);
		return;
	}
	if (g_strcmp0(method, "AccessDialog") == 0) {
		const gchar *app_id = NULL, *title = NULL;
		g_variant_get_child(params, 1, "&s", &app_id);
		g_variant_get_child(params, 3, "&s", &title);
		fprintf(stderr, "portal-npk: access for '%s' (%s): granted\n", app_id, title);
		empty_reply(inv, 0);
		return;
	}
	g_dbus_method_invocation_return_error(inv, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_METHOD, "%s", method);
}

static const GDBusInterfaceVTable vtable = { on_call, on_get_property, NULL, { 0 } };

static void on_bus(GDBusConnection *conn, const gchar *name, gpointer user)
{
	cast.conn = conn;
	const char *names[] = { "org.freedesktop.impl.portal.Access", "org.freedesktop.impl.portal.ScreenCast" };
	for (size_t i = 0; i < G_N_ELEMENTS(names); i++) {
		GError *err = NULL;
		if (g_dbus_connection_register_object(conn, "/org/freedesktop/portal/desktop", iface(names[i]),
						      &vtable, NULL, NULL, &err) == 0) {
			fprintf(stderr, "portal-npk: register %s: %s\n", names[i], err->message);
			g_error_free(err);
		}
	}
}

static void on_name(GDBusConnection *conn, const gchar *name, gpointer user)
{
	fprintf(stderr, "portal-npk: %s\n", name);
}

static gboolean on_pw(gint fd, GIOCondition cond, gpointer user)
{
	pw_loop_iterate(pwloop, 0);
	return G_SOURCE_CONTINUE;
}

int main(void)
{
	pw_init(NULL, NULL);
	pwloop = pw_loop_new(NULL);
	pwctx = pw_context_new(pwloop, NULL, 0);
	pw_loop_enter(pwloop);
	g_unix_fd_add(pw_loop_get_fd(pwloop), G_IO_IN, on_pw, NULL);

	cast.info = g_dbus_node_info_new_for_xml(introspection, NULL);
	GMainLoop *loop = g_main_loop_new(NULL, FALSE);
	g_bus_own_name(G_BUS_TYPE_SESSION, "org.freedesktop.impl.portal.desktop.npk", G_BUS_NAME_OWNER_FLAGS_NONE,
		       on_bus, on_name, NULL, NULL, NULL);
	g_main_loop_run(loop);
	return 0;
}
