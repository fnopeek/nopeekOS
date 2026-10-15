/* npk-gpuprobe: open the virtio-gpu render node the way an EGL client
 * does without a compositor (GBM platform), make a GLES2 context current,
 * print the driver Mesa ended up on, then clear a small renderbuffer to a
 * known colour and read it back: that runs a batch on the card and shows
 * whether its result reaches memory. Run once per guest session when the
 * host offers CONTEXT_INIT; "llvmpipe" means the native context did not
 * come up, and the host log says why. */
#define _GNU_SOURCE
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
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

    GLuint rb, fb;
    glGenRenderbuffers(1, &rb);
    glBindRenderbuffer(GL_RENDERBUFFER, rb);
    glRenderbufferStorage(GL_RENDERBUFFER, GL_RGBA8_OES, 64, 64);
    glGenFramebuffers(1, &fb);
    glBindFramebuffer(GL_FRAMEBUFFER, fb);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, rb);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) {
        printf("<0>[gpu] guest EGL: framebuffer incomplete\n");
        return 1;
    }
    glClearColor(0x40 / 255.0f, 0x80 / 255.0f, 0xc0 / 255.0f, 1.0f);
    glClear(GL_COLOR_BUFFER_BIT);
    unsigned char px[4] = { 0 };
    glReadPixels(32, 32, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, px);
    printf("<0>[gpu] guest EGL: clear and read back %02x%02x%02x%02x (want 4080c0ff), gl error 0x%x, %ld ms\n",
           px[0], px[1], px[2], px[3], glGetError(), ms_since(&t0));
    glDeleteFramebuffers(1, &fb);
    glDeleteRenderbuffers(1, &rb);

    eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroyContext(dpy, ctx);
    eglTerminate(dpy);
    gbm_device_destroy(gbm);
    close(fd);
    return 0;
}
