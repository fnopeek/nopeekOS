/* npk-gpuprobe: open the virtio-gpu render node the way an EGL client
 * does without a compositor (GBM platform), make a GLES2 context current
 * and print one line with the driver Mesa ended up on. Run once per guest
 * session when the host offers CONTEXT_INIT; "llvmpipe" there means the
 * native context did not come up, and the host log says why. */
#define _GNU_SOURCE
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <fcntl.h>
#include <gbm.h>
#include <stdio.h>
#include <time.h>
#include <unistd.h>
#include <xf86drm.h>

static long ms_since(const struct timespec *t0)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (t.tv_sec - t0->tv_sec) * 1000 + (t.tv_nsec - t0->tv_nsec) / 1000000;
}

static int fail(const char *step, const struct timespec *t0)
{
    printf("<0>[gpu] guest EGL: failed at %s (egl 0x%x) after %ld ms\n", step, eglGetError(), ms_since(t0));
    return 1;
}

int main(int argc, char **argv)
{
    const char *node = argc > 1 ? argv[1] : "/dev/dri/renderD128";
    struct timespec t0;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    setvbuf(stdout, NULL, _IOLBF, 0);

    int fd = open(node, O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        printf("<0>[gpu] guest EGL: cannot open %s\n", node);
        return 1;
    }
    drmVersionPtr v = drmGetVersion(fd);
    printf("<0>[gpu] guest EGL: %s is %s\n", node, v ? v->name : "?");
    drmFreeVersion(v);

    struct gbm_device *gbm = gbm_create_device(fd);
    if (!gbm)
        return fail("gbm_create_device", &t0);
    EGLDisplay dpy = eglGetPlatformDisplay(EGL_PLATFORM_GBM_KHR, gbm, NULL);
    if (dpy == EGL_NO_DISPLAY)
        return fail("eglGetPlatformDisplay", &t0);
    if (!eglInitialize(dpy, NULL, NULL))
        return fail("eglInitialize", &t0);
    if (!eglBindAPI(EGL_OPENGL_ES_API))
        return fail("eglBindAPI", &t0);
    static const EGLint attrs[] = { EGL_CONTEXT_MAJOR_VERSION, 2, EGL_NONE };
    EGLContext ctx = eglCreateContext(dpy, EGL_NO_CONFIG_KHR, EGL_NO_CONTEXT, attrs);
    if (ctx == EGL_NO_CONTEXT)
        return fail("eglCreateContext", &t0);
    if (!eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, ctx))
        return fail("eglMakeCurrent", &t0);

    printf("<0>[gpu] guest EGL: renderer \"%s\", %s, %ld ms\n",
           (const char *)glGetString(GL_RENDERER), (const char *)glGetString(GL_VERSION), ms_since(&t0));

    eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroyContext(dpy, ctx);
    eglTerminate(dpy);
    gbm_device_destroy(gbm);
    close(fd);
    return 0;
}
