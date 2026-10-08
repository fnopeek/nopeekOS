/*
 * npkwm: the Wayland compositor inside a nopeekOS app VM.
 *
 * Built on wlroots' tinywl (CC0). One output, the host window. A toplevel
 * without a parent fills the output; a dialog keeps its own size, centred.
 * The host talks to it over the virtio-console port "npk.windows"
 * (`NPK_WINDOWS` names the device): the host asks to close the app's
 * windows, npkwm reports the title of the focused one.
 *
 * Frames on that port: u16 len | u8 type | payload, little-endian, `len`
 * counting the type byte and the payload, at most WIN_MAX_FRAME.
 */

#define _POSIX_C_SOURCE 200809L

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <wayland-server-core.h>
#include <wlr/backend.h>
#include <wlr/render/allocator.h>
#include <wlr/render/wlr_renderer.h>
#include <wlr/types/wlr_compositor.h>
#include <wlr/types/wlr_cursor.h>
#include <wlr/types/wlr_data_device.h>
#include <wlr/types/wlr_input_device.h>
#include <wlr/types/wlr_keyboard.h>
#include <wlr/types/wlr_output.h>
#include <wlr/types/wlr_output_layout.h>
#include <wlr/types/wlr_pointer.h>
#include <wlr/types/wlr_presentation_time.h>
#include <wlr/types/wlr_primary_selection.h>
#include <wlr/types/wlr_primary_selection_v1.h>
#include <wlr/types/wlr_scene.h>
#include <wlr/types/wlr_seat.h>
#include <wlr/types/wlr_single_pixel_buffer_v1.h>
#include <wlr/types/wlr_subcompositor.h>
#include <wlr/types/wlr_viewporter.h>
#include <wlr/types/wlr_xcursor_manager.h>
#include <wlr/types/wlr_xdg_output_v1.h>
#include <wlr/types/wlr_xdg_shell.h>
#include <wlr/util/log.h>
#include <xkbcommon/xkbcommon.h>

/* Host -> guest. */
#define WIN_CLOSE 0x01
/* Guest -> host. */
#define WIN_HELLO 0x81
#define WIN_TITLE 0x82

#define WIN_VERSION 1
#define WIN_MAX_FRAME 512
/* Bytes of title sent; cut on a UTF-8 boundary. */
#define TITLE_MAX 200

struct server {
	struct wl_display *display;
	struct wlr_backend *backend;
	struct wlr_renderer *renderer;
	struct wlr_allocator *allocator;
	struct wlr_scene *scene;
	struct wlr_scene_output_layout *scene_layout;
	struct wlr_output_layout *output_layout;
	struct wl_listener layout_change;
	struct wl_listener new_output;

	struct wlr_xdg_shell *xdg_shell;
	struct wl_listener new_xdg_toplevel;
	struct wl_listener new_xdg_popup;
	/* Mapped toplevels, most recently focused first. */
	struct wl_list toplevels;

	struct wlr_cursor *cursor;
	struct wlr_xcursor_manager *cursor_mgr;
	struct wl_listener cursor_motion;
	struct wl_listener cursor_motion_absolute;
	struct wl_listener cursor_button;
	struct wl_listener cursor_axis;
	struct wl_listener cursor_frame;

	struct wlr_seat *seat;
	struct wl_listener new_input;
	struct wl_listener request_cursor;
	struct wl_listener request_set_selection;
	struct wl_listener request_set_primary_selection;
	struct wl_list keyboards;

	/* npk.windows; -1 without a host channel. */
	int win_fd;
	uint8_t win_rx[2 + WIN_MAX_FRAME];
	size_t win_rx_len;
	char sent_title[TITLE_MAX + 1];
};

struct output {
	struct server *server;
	struct wlr_output *wlr_output;
	struct wl_listener frame;
	struct wl_listener request_state;
	struct wl_listener destroy;
};

struct toplevel {
	struct wl_list link;
	struct server *server;
	struct wlr_xdg_toplevel *xdg_toplevel;
	struct wlr_scene_tree *scene_tree;
	struct wl_listener map;
	struct wl_listener unmap;
	struct wl_listener commit;
	struct wl_listener destroy;
	struct wl_listener request_maximize;
	struct wl_listener request_fullscreen;
	struct wl_listener set_title;
};

struct popup {
	struct wlr_xdg_popup *xdg_popup;
	struct wl_listener commit;
	struct wl_listener reposition;
	struct wl_listener destroy;
};

struct keyboard {
	struct wl_list link;
	struct server *server;
	struct wlr_keyboard *wlr_keyboard;
	struct wl_listener modifiers;
	struct wl_listener key;
	struct wl_listener destroy;
};

/* ── npk.windows ─────────────────────────────────────────────────── */

static void win_send(struct server *server, uint8_t type, const void *payload, size_t len) {
	if (server->win_fd < 0 || len + 1 > WIN_MAX_FRAME) {
		return;
	}
	uint8_t frame[2 + WIN_MAX_FRAME];
	uint16_t flen = (uint16_t)(len + 1);
	frame[0] = flen & 0xff;
	frame[1] = flen >> 8;
	frame[2] = type;
	memcpy(frame + 3, payload, len);
	/* One write per frame; a full port drops it rather than block. */
	if (write(server->win_fd, frame, 3 + len) < 0 && errno != EAGAIN) {
		fprintf(stderr, "npkwm: npk.windows write: %s\n", strerror(errno));
	}
}

static struct toplevel *focused_toplevel(struct server *server) {
	if (wl_list_empty(&server->toplevels)) {
		return NULL;
	}
	struct toplevel *t = wl_container_of(server->toplevels.next, t, link);
	return t;
}

/* Report the focused toplevel's title, if it changed. Payload: u8 output
 * index, then the title bytes. */
static void report_title(struct server *server) {
	struct toplevel *t = focused_toplevel(server);
	const char *title = (t && t->xdg_toplevel->title) ? t->xdg_toplevel->title : "";
	size_t n = strlen(title);
	if (n > TITLE_MAX) {
		n = TITLE_MAX;
		while (n > 0 && ((unsigned char)title[n] & 0xc0) == 0x80) {
			n--;
		}
	}
	if (strlen(server->sent_title) == n && memcmp(server->sent_title, title, n) == 0) {
		return;
	}
	memcpy(server->sent_title, title, n);
	server->sent_title[n] = '\0';
	uint8_t payload[1 + TITLE_MAX];
	payload[0] = 0;
	memcpy(payload + 1, title, n);
	win_send(server, WIN_TITLE, payload, 1 + n);
}

static void close_all(struct server *server) {
	struct toplevel *t;
	wl_list_for_each(t, &server->toplevels, link) {
		if (t->xdg_toplevel->parent == NULL) {
			wlr_xdg_toplevel_send_close(t->xdg_toplevel);
		}
	}
}

static void win_handle_frame(struct server *server, uint8_t type, const uint8_t *payload, size_t len) {
	switch (type) {
	case WIN_CLOSE:
		close_all(server);
		break;
	default:
		break;
	}
}

static int win_readable(int fd, uint32_t mask, void *data) {
	struct server *server = data;
	if (mask & (WL_EVENT_HANGUP | WL_EVENT_ERROR)) {
		return 0;
	}
	ssize_t n = read(fd, server->win_rx + server->win_rx_len,
		sizeof(server->win_rx) - server->win_rx_len);
	if (n <= 0) {
		return 0;
	}
	server->win_rx_len += (size_t)n;
	while (server->win_rx_len >= 2) {
		size_t len = server->win_rx[0] | ((size_t)server->win_rx[1] << 8);
		if (len == 0 || len > WIN_MAX_FRAME) {
			server->win_rx_len = 0;
			break;
		}
		if (server->win_rx_len < 2 + len) {
			break;
		}
		win_handle_frame(server, server->win_rx[2], server->win_rx + 3, len - 1);
		memmove(server->win_rx, server->win_rx + 2 + len, server->win_rx_len - 2 - len);
		server->win_rx_len -= 2 + len;
	}
	return 0;
}

static void win_open(struct server *server) {
	server->win_fd = -1;
	const char *path = getenv("NPK_WINDOWS");
	if (path == NULL || *path == '\0') {
		fprintf(stderr, "npkwm: no NPK_WINDOWS, running without the host channel\n");
		return;
	}
	server->win_fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
	if (server->win_fd < 0) {
		fprintf(stderr, "npkwm: %s: %s\n", path, strerror(errno));
		return;
	}
	wl_event_loop_add_fd(wl_display_get_event_loop(server->display), server->win_fd,
		WL_EVENT_READABLE, win_readable, server);
	uint8_t version = WIN_VERSION;
	win_send(server, WIN_HELLO, &version, 1);
}

/* ── Layout ──────────────────────────────────────────────────────── */

static struct wlr_box output_box(struct server *server) {
	struct wlr_box box = {0};
	wlr_output_layout_get_box(server->output_layout, NULL, &box);
	return box;
}

/* A toplevel without a parent fills the output; a dialog is centred. */
static void place_toplevel(struct toplevel *t) {
	struct server *server = t->server;
	struct wlr_box out = output_box(server);
	if (t->xdg_toplevel->parent == NULL) {
		wlr_scene_node_set_position(&t->scene_tree->node, out.x, out.y);
		if (t->xdg_toplevel->base->initialized) {
			wlr_xdg_toplevel_set_size(t->xdg_toplevel, out.width, out.height);
		}
		return;
	}
	struct wlr_box *geo = &t->xdg_toplevel->base->geometry;
	int x = out.x + (out.width - geo->width) / 2 - geo->x;
	int y = out.y + (out.height - geo->height) / 2 - geo->y;
	wlr_scene_node_set_position(&t->scene_tree->node, x < out.x ? out.x : x, y < out.y ? out.y : y);
}

static void layout_change(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, layout_change);
	struct toplevel *t;
	wl_list_for_each(t, &server->toplevels, link) {
		place_toplevel(t);
	}
}

/* ── Focus ───────────────────────────────────────────────────────── */

static void focus_toplevel(struct toplevel *t) {
	if (t == NULL) {
		return;
	}
	struct server *server = t->server;
	struct wlr_seat *seat = server->seat;
	struct wlr_surface *prev = seat->keyboard_state.focused_surface;
	struct wlr_surface *surface = t->xdg_toplevel->base->surface;
	if (prev != surface) {
		if (prev) {
			struct wlr_xdg_toplevel *prev_toplevel = wlr_xdg_toplevel_try_from_wlr_surface(prev);
			if (prev_toplevel != NULL) {
				wlr_xdg_toplevel_set_activated(prev_toplevel, false);
			}
		}
		wlr_scene_node_raise_to_top(&t->scene_tree->node);
		wl_list_remove(&t->link);
		wl_list_insert(&server->toplevels, &t->link);
		wlr_xdg_toplevel_set_activated(t->xdg_toplevel, true);
		struct wlr_keyboard *keyboard = wlr_seat_get_keyboard(seat);
		if (keyboard != NULL) {
			wlr_seat_keyboard_notify_enter(seat, surface,
				keyboard->keycodes, keyboard->num_keycodes, &keyboard->modifiers);
		}
	}
	report_title(server);
}

/* ── Keyboard ────────────────────────────────────────────────────── */

static void keyboard_modifiers(struct wl_listener *listener, void *data) {
	struct keyboard *keyboard = wl_container_of(listener, keyboard, modifiers);
	wlr_seat_set_keyboard(keyboard->server->seat, keyboard->wlr_keyboard);
	wlr_seat_keyboard_notify_modifiers(keyboard->server->seat, &keyboard->wlr_keyboard->modifiers);
}

static void keyboard_key(struct wl_listener *listener, void *data) {
	struct keyboard *keyboard = wl_container_of(listener, keyboard, key);
	struct wlr_keyboard_key_event *event = data;
	wlr_seat_set_keyboard(keyboard->server->seat, keyboard->wlr_keyboard);
	wlr_seat_keyboard_notify_key(keyboard->server->seat, event->time_msec, event->keycode, event->state);
}

static void keyboard_destroy(struct wl_listener *listener, void *data) {
	struct keyboard *keyboard = wl_container_of(listener, keyboard, destroy);
	wl_list_remove(&keyboard->modifiers.link);
	wl_list_remove(&keyboard->key.link);
	wl_list_remove(&keyboard->destroy.link);
	wl_list_remove(&keyboard->link);
	free(keyboard);
}

static void new_keyboard(struct server *server, struct wlr_input_device *device) {
	struct wlr_keyboard *wlr_keyboard = wlr_keyboard_from_input_device(device);
	struct keyboard *keyboard = calloc(1, sizeof(*keyboard));
	if (keyboard == NULL) {
		return;
	}
	keyboard->server = server;
	keyboard->wlr_keyboard = wlr_keyboard;

	/* NULL names: the layout comes from the XKB_DEFAULT_* environment. */
	struct xkb_context *context = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
	struct xkb_keymap *keymap = xkb_keymap_new_from_names(context, NULL, XKB_KEYMAP_COMPILE_NO_FLAGS);
	wlr_keyboard_set_keymap(wlr_keyboard, keymap);
	xkb_keymap_unref(keymap);
	xkb_context_unref(context);
	wlr_keyboard_set_repeat_info(wlr_keyboard, 25, 600);

	keyboard->modifiers.notify = keyboard_modifiers;
	wl_signal_add(&wlr_keyboard->events.modifiers, &keyboard->modifiers);
	keyboard->key.notify = keyboard_key;
	wl_signal_add(&wlr_keyboard->events.key, &keyboard->key);
	keyboard->destroy.notify = keyboard_destroy;
	wl_signal_add(&device->events.destroy, &keyboard->destroy);

	wlr_seat_set_keyboard(server->seat, wlr_keyboard);
	wl_list_insert(&server->keyboards, &keyboard->link);
}

static void new_input(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, new_input);
	struct wlr_input_device *device = data;
	switch (device->type) {
	case WLR_INPUT_DEVICE_KEYBOARD:
		new_keyboard(server, device);
		break;
	case WLR_INPUT_DEVICE_POINTER:
		wlr_cursor_attach_input_device(server->cursor, device);
		break;
	default:
		break;
	}
	uint32_t caps = WL_SEAT_CAPABILITY_POINTER;
	if (!wl_list_empty(&server->keyboards)) {
		caps |= WL_SEAT_CAPABILITY_KEYBOARD;
	}
	wlr_seat_set_capabilities(server->seat, caps);
}

/* ── Seat requests ───────────────────────────────────────────────── */

static void request_cursor(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, request_cursor);
	struct wlr_seat_pointer_request_set_cursor_event *event = data;
	if (server->seat->pointer_state.focused_client == event->seat_client) {
		wlr_cursor_set_surface(server->cursor, event->surface, event->hotspot_x, event->hotspot_y);
	}
}

static void request_set_selection(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, request_set_selection);
	struct wlr_seat_request_set_selection_event *event = data;
	wlr_seat_set_selection(server->seat, event->source, event->serial);
}

static void request_set_primary_selection(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, request_set_primary_selection);
	struct wlr_seat_request_set_primary_selection_event *event = data;
	wlr_seat_set_primary_selection(server->seat, event->source, event->serial);
}

/* ── Pointer ─────────────────────────────────────────────────────── */

static struct toplevel *toplevel_at(struct server *server, double lx, double ly,
		struct wlr_surface **surface, double *sx, double *sy) {
	struct wlr_scene_node *node = wlr_scene_node_at(&server->scene->tree.node, lx, ly, sx, sy);
	if (node == NULL || node->type != WLR_SCENE_NODE_BUFFER) {
		return NULL;
	}
	struct wlr_scene_surface *scene_surface =
		wlr_scene_surface_try_from_buffer(wlr_scene_buffer_from_node(node));
	if (scene_surface == NULL) {
		return NULL;
	}
	*surface = scene_surface->surface;
	struct wlr_scene_tree *tree = node->parent;
	while (tree != NULL && tree->node.data == NULL) {
		tree = tree->node.parent;
	}
	return tree ? tree->node.data : NULL;
}

static void process_motion(struct server *server, uint32_t time) {
	double sx, sy;
	struct wlr_surface *surface = NULL;
	struct toplevel *t = toplevel_at(server, server->cursor->x, server->cursor->y, &surface, &sx, &sy);
	if (t == NULL) {
		wlr_cursor_set_xcursor(server->cursor, server->cursor_mgr, "default");
	}
	if (surface) {
		wlr_seat_pointer_notify_enter(server->seat, surface, sx, sy);
		wlr_seat_pointer_notify_motion(server->seat, time, sx, sy);
	} else {
		wlr_seat_pointer_clear_focus(server->seat);
	}
}

static void cursor_motion(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, cursor_motion);
	struct wlr_pointer_motion_event *event = data;
	wlr_cursor_move(server->cursor, &event->pointer->base, event->delta_x, event->delta_y);
	process_motion(server, event->time_msec);
}

static void cursor_motion_absolute(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, cursor_motion_absolute);
	struct wlr_pointer_motion_absolute_event *event = data;
	wlr_cursor_warp_absolute(server->cursor, &event->pointer->base, event->x, event->y);
	process_motion(server, event->time_msec);
}

static void cursor_button(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, cursor_button);
	struct wlr_pointer_button_event *event = data;
	wlr_seat_pointer_notify_button(server->seat, event->time_msec, event->button, event->state);
	if (event->state == WL_POINTER_BUTTON_STATE_PRESSED) {
		double sx, sy;
		struct wlr_surface *surface = NULL;
		focus_toplevel(toplevel_at(server, server->cursor->x, server->cursor->y, &surface, &sx, &sy));
	}
}

static void cursor_axis(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, cursor_axis);
	struct wlr_pointer_axis_event *event = data;
	wlr_seat_pointer_notify_axis(server->seat, event->time_msec, event->orientation,
		event->delta, event->delta_discrete, event->source, event->relative_direction);
}

static void cursor_frame(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, cursor_frame);
	wlr_seat_pointer_notify_frame(server->seat);
}

/* ── Outputs ─────────────────────────────────────────────────────── */

static void output_frame(struct wl_listener *listener, void *data) {
	struct output *output = wl_container_of(listener, output, frame);
	struct wlr_scene_output *scene_output =
		wlr_scene_get_scene_output(output->server->scene, output->wlr_output);
	if (scene_output == NULL) {
		return;
	}
	wlr_scene_output_commit(scene_output, NULL);
	struct timespec now;
	clock_gettime(CLOCK_MONOTONIC, &now);
	wlr_scene_output_send_frame_done(scene_output, &now);
}

static void output_request_state(struct wl_listener *listener, void *data) {
	struct output *output = wl_container_of(listener, output, request_state);
	const struct wlr_output_event_request_state *event = data;
	wlr_output_commit_state(output->wlr_output, event->state);
}

static void output_destroy(struct wl_listener *listener, void *data) {
	struct output *output = wl_container_of(listener, output, destroy);
	wl_list_remove(&output->frame.link);
	wl_list_remove(&output->request_state.link);
	wl_list_remove(&output->destroy.link);
	free(output);
}

static void new_output(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, new_output);
	struct wlr_output *wlr_output = data;
	if (wlr_output->non_desktop || !wlr_output_init_render(wlr_output, server->allocator, server->renderer)) {
		return;
	}
	struct output *output = calloc(1, sizeof(*output));
	if (output == NULL) {
		return;
	}
	output->server = server;
	output->wlr_output = wlr_output;
	output->frame.notify = output_frame;
	wl_signal_add(&wlr_output->events.frame, &output->frame);
	output->request_state.notify = output_request_state;
	wl_signal_add(&wlr_output->events.request_state, &output->request_state);
	output->destroy.notify = output_destroy;
	wl_signal_add(&wlr_output->events.destroy, &output->destroy);

	struct wlr_output_state state;
	wlr_output_state_init(&state);
	wlr_output_state_set_enabled(&state, true);
	struct wlr_output_mode *mode = wlr_output_preferred_mode(wlr_output);
	if (mode != NULL) {
		wlr_output_state_set_mode(&state, mode);
	}
	wlr_output_commit_state(wlr_output, &state);
	wlr_output_state_finish(&state);

	struct wlr_output_layout_output *l_output = wlr_output_layout_add_auto(server->output_layout, wlr_output);
	struct wlr_scene_output *scene_output = wlr_scene_output_create(server->scene, wlr_output);
	wlr_scene_output_layout_add_output(server->scene_layout, l_output, scene_output);
}

/* ── Toplevels ───────────────────────────────────────────────────── */

static void toplevel_map(struct wl_listener *listener, void *data) {
	struct toplevel *t = wl_container_of(listener, t, map);
	wl_list_insert(&t->server->toplevels, &t->link);
	place_toplevel(t);
	focus_toplevel(t);
}

static void toplevel_unmap(struct wl_listener *listener, void *data) {
	struct toplevel *t = wl_container_of(listener, t, unmap);
	struct server *server = t->server;
	bool was_focused = focused_toplevel(server) == t;
	wl_list_remove(&t->link);
	if (was_focused) {
		struct toplevel *next = focused_toplevel(server);
		if (next) {
			focus_toplevel(next);
		} else {
			report_title(server);
		}
	}
}

static void toplevel_commit(struct wl_listener *listener, void *data) {
	struct toplevel *t = wl_container_of(listener, t, commit);
	if (!t->xdg_toplevel->base->initial_commit) {
		return;
	}
	if (t->xdg_toplevel->parent == NULL) {
		struct wlr_box out = output_box(t->server);
		wlr_xdg_toplevel_set_size(t->xdg_toplevel, out.width, out.height);
		wlr_xdg_toplevel_set_maximized(t->xdg_toplevel, true);
	} else {
		wlr_xdg_toplevel_set_size(t->xdg_toplevel, 0, 0);
	}
}

static void toplevel_request_maximize(struct wl_listener *listener, void *data) {
	struct toplevel *t = wl_container_of(listener, t, request_maximize);
	if (t->xdg_toplevel->base->initialized) {
		wlr_xdg_surface_schedule_configure(t->xdg_toplevel->base);
	}
}

static void toplevel_request_fullscreen(struct wl_listener *listener, void *data) {
	struct toplevel *t = wl_container_of(listener, t, request_fullscreen);
	if (t->xdg_toplevel->base->initialized) {
		wlr_xdg_toplevel_set_fullscreen(t->xdg_toplevel, t->xdg_toplevel->requested.fullscreen);
	}
}

static void toplevel_set_title(struct wl_listener *listener, void *data) {
	struct toplevel *t = wl_container_of(listener, t, set_title);
	if (focused_toplevel(t->server) == t && t->xdg_toplevel->base->surface->mapped) {
		report_title(t->server);
	}
}

static void toplevel_destroy(struct wl_listener *listener, void *data) {
	struct toplevel *t = wl_container_of(listener, t, destroy);
	wl_list_remove(&t->map.link);
	wl_list_remove(&t->unmap.link);
	wl_list_remove(&t->commit.link);
	wl_list_remove(&t->destroy.link);
	wl_list_remove(&t->request_maximize.link);
	wl_list_remove(&t->request_fullscreen.link);
	wl_list_remove(&t->set_title.link);
	free(t);
}

static void new_xdg_toplevel(struct wl_listener *listener, void *data) {
	struct server *server = wl_container_of(listener, server, new_xdg_toplevel);
	struct wlr_xdg_toplevel *xdg_toplevel = data;
	struct toplevel *t = calloc(1, sizeof(*t));
	if (t == NULL) {
		return;
	}
	t->server = server;
	t->xdg_toplevel = xdg_toplevel;
	t->scene_tree = wlr_scene_xdg_surface_create(&server->scene->tree, xdg_toplevel->base);
	t->scene_tree->node.data = t;
	xdg_toplevel->base->data = t->scene_tree;

	t->map.notify = toplevel_map;
	wl_signal_add(&xdg_toplevel->base->surface->events.map, &t->map);
	t->unmap.notify = toplevel_unmap;
	wl_signal_add(&xdg_toplevel->base->surface->events.unmap, &t->unmap);
	t->commit.notify = toplevel_commit;
	wl_signal_add(&xdg_toplevel->base->surface->events.commit, &t->commit);
	t->destroy.notify = toplevel_destroy;
	wl_signal_add(&xdg_toplevel->events.destroy, &t->destroy);
	t->request_maximize.notify = toplevel_request_maximize;
	wl_signal_add(&xdg_toplevel->events.request_maximize, &t->request_maximize);
	t->request_fullscreen.notify = toplevel_request_fullscreen;
	wl_signal_add(&xdg_toplevel->events.request_fullscreen, &t->request_fullscreen);
	t->set_title.notify = toplevel_set_title;
	wl_signal_add(&xdg_toplevel->events.set_title, &t->set_title);
}

/* ── Popups ──────────────────────────────────────────────────────── */

/* Keep a popup inside the output, in coordinates of its toplevel. */
static void popup_unconstrain(struct wlr_xdg_popup *popup) {
	struct wlr_xdg_popup *p = popup;
	struct wlr_xdg_surface *parent = NULL;
	while ((parent = wlr_xdg_surface_try_from_wlr_surface(p->parent)) != NULL
			&& parent->role == WLR_XDG_SURFACE_ROLE_POPUP) {
		p = parent->popup;
	}
	if (parent == NULL || parent->role != WLR_XDG_SURFACE_ROLE_TOPLEVEL || parent->data == NULL) {
		return;
	}
	struct wlr_scene_tree *tree = parent->data;
	struct toplevel *t = tree->node.data;
	struct wlr_box out = output_box(t->server);
	struct wlr_box box = {
		.x = out.x - tree->node.x,
		.y = out.y - tree->node.y,
		.width = out.width,
		.height = out.height,
	};
	wlr_xdg_popup_unconstrain_from_box(popup, &box);
}

static void popup_commit(struct wl_listener *listener, void *data) {
	struct popup *popup = wl_container_of(listener, popup, commit);
	if (popup->xdg_popup->base->initial_commit) {
		popup_unconstrain(popup->xdg_popup);
		wlr_xdg_surface_schedule_configure(popup->xdg_popup->base);
	}
}

static void popup_reposition(struct wl_listener *listener, void *data) {
	struct popup *popup = wl_container_of(listener, popup, reposition);
	popup_unconstrain(popup->xdg_popup);
}

static void popup_destroy(struct wl_listener *listener, void *data) {
	struct popup *popup = wl_container_of(listener, popup, destroy);
	wl_list_remove(&popup->commit.link);
	wl_list_remove(&popup->reposition.link);
	wl_list_remove(&popup->destroy.link);
	free(popup);
}

static void new_xdg_popup(struct wl_listener *listener, void *data) {
	struct wlr_xdg_popup *xdg_popup = data;
	struct wlr_xdg_surface *parent = wlr_xdg_surface_try_from_wlr_surface(xdg_popup->parent);
	if (parent == NULL || parent->data == NULL) {
		return;
	}
	struct popup *popup = calloc(1, sizeof(*popup));
	if (popup == NULL) {
		return;
	}
	popup->xdg_popup = xdg_popup;
	xdg_popup->base->data = wlr_scene_xdg_surface_create(parent->data, xdg_popup->base);
	popup->commit.notify = popup_commit;
	wl_signal_add(&xdg_popup->base->surface->events.commit, &popup->commit);
	popup->reposition.notify = popup_reposition;
	wl_signal_add(&xdg_popup->events.reposition, &popup->reposition);
	popup->destroy.notify = popup_destroy;
	wl_signal_add(&xdg_popup->events.destroy, &popup->destroy);
}

/* ── Main ────────────────────────────────────────────────────────── */

static int handle_signal(int signal, void *data) {
	wl_display_terminate(data);
	return 0;
}

int main(void) {
	wlr_log_init(WLR_ERROR, NULL);
	struct server server = {0};
	server.win_fd = -1;
	wl_list_init(&server.toplevels);
	wl_list_init(&server.keyboards);

	server.display = wl_display_create();
	struct wl_event_loop *loop = wl_display_get_event_loop(server.display);
	wl_event_loop_add_signal(loop, SIGTERM, handle_signal, server.display);
	wl_event_loop_add_signal(loop, SIGINT, handle_signal, server.display);

	server.backend = wlr_backend_autocreate(loop, NULL);
	if (server.backend == NULL) {
		fprintf(stderr, "npkwm: no backend\n");
		return 1;
	}
	server.renderer = wlr_renderer_autocreate(server.backend);
	if (server.renderer == NULL) {
		fprintf(stderr, "npkwm: no renderer\n");
		return 1;
	}
	wlr_renderer_init_wl_display(server.renderer, server.display);
	server.allocator = wlr_allocator_autocreate(server.backend, server.renderer);
	if (server.allocator == NULL) {
		fprintf(stderr, "npkwm: no allocator\n");
		return 1;
	}

	wlr_compositor_create(server.display, 6, server.renderer);
	wlr_subcompositor_create(server.display);
	wlr_data_device_manager_create(server.display);
	wlr_primary_selection_v1_device_manager_create(server.display);
	wlr_viewporter_create(server.display);
	wlr_single_pixel_buffer_manager_v1_create(server.display);
	wlr_presentation_create(server.display, server.backend, 2);

	server.output_layout = wlr_output_layout_create(server.display);
	wlr_xdg_output_manager_v1_create(server.display, server.output_layout);
	server.layout_change.notify = layout_change;
	wl_signal_add(&server.output_layout->events.change, &server.layout_change);
	server.new_output.notify = new_output;
	wl_signal_add(&server.backend->events.new_output, &server.new_output);

	server.scene = wlr_scene_create();
	server.scene_layout = wlr_scene_attach_output_layout(server.scene, server.output_layout);

	server.xdg_shell = wlr_xdg_shell_create(server.display, 5);
	server.new_xdg_toplevel.notify = new_xdg_toplevel;
	wl_signal_add(&server.xdg_shell->events.new_toplevel, &server.new_xdg_toplevel);
	server.new_xdg_popup.notify = new_xdg_popup;
	wl_signal_add(&server.xdg_shell->events.new_popup, &server.new_xdg_popup);

	server.cursor = wlr_cursor_create();
	wlr_cursor_attach_output_layout(server.cursor, server.output_layout);
	server.cursor_mgr = wlr_xcursor_manager_create(NULL, 24);
	server.cursor_motion.notify = cursor_motion;
	wl_signal_add(&server.cursor->events.motion, &server.cursor_motion);
	server.cursor_motion_absolute.notify = cursor_motion_absolute;
	wl_signal_add(&server.cursor->events.motion_absolute, &server.cursor_motion_absolute);
	server.cursor_button.notify = cursor_button;
	wl_signal_add(&server.cursor->events.button, &server.cursor_button);
	server.cursor_axis.notify = cursor_axis;
	wl_signal_add(&server.cursor->events.axis, &server.cursor_axis);
	server.cursor_frame.notify = cursor_frame;
	wl_signal_add(&server.cursor->events.frame, &server.cursor_frame);

	server.new_input.notify = new_input;
	wl_signal_add(&server.backend->events.new_input, &server.new_input);
	server.seat = wlr_seat_create(server.display, "seat0");
	server.request_cursor.notify = request_cursor;
	wl_signal_add(&server.seat->events.request_set_cursor, &server.request_cursor);
	server.request_set_selection.notify = request_set_selection;
	wl_signal_add(&server.seat->events.request_set_selection, &server.request_set_selection);
	server.request_set_primary_selection.notify = request_set_primary_selection;
	wl_signal_add(&server.seat->events.request_set_primary_selection,
		&server.request_set_primary_selection);

	if (!wlr_backend_start(server.backend)) {
		fprintf(stderr, "npkwm: backend did not start\n");
		return 1;
	}

	/* The socket is reachable by the app's group: the runtime dir is
	 * setgid to that group and the socket is made group-writable. */
	if (wl_display_add_socket(server.display, "wayland-0") != 0) {
		fprintf(stderr, "npkwm: no socket\n");
		return 1;
	}
	const char *runtime = getenv("XDG_RUNTIME_DIR");
	if (runtime != NULL) {
		char path[256];
		snprintf(path, sizeof(path), "%s/wayland-0", runtime);
		chmod(path, 0660);
	}

	win_open(&server);
	fprintf(stderr, "npkwm: running\n");
	wl_display_run(server.display);

	wl_display_destroy_clients(server.display);
	wlr_scene_node_destroy(&server.scene->tree.node);
	wlr_xcursor_manager_destroy(server.cursor_mgr);
	wlr_cursor_destroy(server.cursor);
	wlr_allocator_destroy(server.allocator);
	wlr_renderer_destroy(server.renderer);
	wlr_backend_destroy(server.backend);
	wl_display_destroy(server.display);
	return 0;
}
