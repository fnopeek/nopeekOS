/*
 * portal-npk: nopeekOS's xdg-desktop-portal backend in the guest.
 *
 * xdg-desktop-portal offers the Camera portal only when a backend
 * implements org.freedesktop.impl.portal.Access. This one does, and grants:
 * the browser asks the user before a page gets the camera, and the host
 * shows while the camera runs. The screen share's ScreenCast comes here
 * later.
 *
 * D-Bus name org.freedesktop.impl.portal.desktop.npk, activated by the
 * session bus; npk.portal and portals.conf tell xdg-desktop-portal to use it.
 */
#include <gio/gio.h>
#include <stdio.h>

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
	"</node>";

static void on_call(GDBusConnection *conn, const gchar *sender, const gchar *path,
		    const gchar *iface, const gchar *method, GVariant *params,
		    GDBusMethodInvocation *inv, gpointer user)
{
	if (g_strcmp0(method, "AccessDialog") == 0) {
		const gchar *app_id = NULL, *title = NULL;
		g_variant_get_child(params, 1, "&s", &app_id);
		g_variant_get_child(params, 3, "&s", &title);
		fprintf(stderr, "portal-npk: access for '%s' (%s): granted\n", app_id, title);
		GVariantBuilder results;
		g_variant_builder_init(&results, G_VARIANT_TYPE_VARDICT);
		g_dbus_method_invocation_return_value(inv, g_variant_new("(ua{sv})", 0, &results));
		return;
	}
	g_dbus_method_invocation_return_error(inv, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_METHOD, "%s", method);
}

static const GDBusInterfaceVTable vtable = { on_call, NULL, NULL, { 0 } };

static void on_bus(GDBusConnection *conn, const gchar *name, gpointer user)
{
	GDBusNodeInfo *info = user;
	GError *err = NULL;
	if (g_dbus_connection_register_object(conn, "/org/freedesktop/portal/desktop", info->interfaces[0],
					      &vtable, NULL, NULL, &err) == 0) {
		fprintf(stderr, "portal-npk: register: %s\n", err->message);
		g_error_free(err);
	}
}

static void on_name(GDBusConnection *conn, const gchar *name, gpointer user)
{
	fprintf(stderr, "portal-npk: %s\n", name);
}

int main(void)
{
	GDBusNodeInfo *info = g_dbus_node_info_new_for_xml(introspection, NULL);
	GMainLoop *loop = g_main_loop_new(NULL, FALSE);
	g_bus_own_name(G_BUS_TYPE_SESSION, "org.freedesktop.impl.portal.desktop.npk", G_BUS_NAME_OWNER_FLAGS_NONE,
		       on_bus, on_name, NULL, info, NULL);
	g_main_loop_run(loop);
	return 0;
}
