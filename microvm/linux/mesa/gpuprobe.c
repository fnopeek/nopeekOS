/* npk-gpuprobe: open the virtio-gpu render node the way an EGL client
 * does without a compositor (GBM platform), make a GLES2 context current,
 * print the driver Mesa ended up on, then clear a small renderbuffer to a
 * known colour and read it back: that runs a batch on the card and shows
 * whether its result reaches memory. Run once per guest session when the
 * host offers CONTEXT_INIT; "llvmpipe" means the native context did not
 * come up, and the host log says why.
 *
 * With `npkgpushare` on the kernel command line, then the path a Wayland
 * client and its compositor take: for each layout (linear, X, Y, Y with
 * render compression and its clear colour, Y with render compression) one
 * buffer is drawn in a first context and read back there, then imported
 * through its dma-buf into a second context on its own fd and read back
 * again. The second read is what the compositor samples. A line before
 * each step names the step, so a hang shows where it stopped. */
#define _GNU_SOURCE
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <fcntl.h>
#include <drm_fourcc.h>
#include <gbm.h>
#include <string.h>
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

struct side {
    int fd;
    struct gbm_device *gbm;
    EGLDisplay dpy;
    EGLContext ctx;
};

static PFNEGLCREATEIMAGEKHRPROC create_image;
static PFNEGLDESTROYIMAGEKHRPROC destroy_image;
static PFNGLEGLIMAGETARGETRENDERBUFFERSTORAGEOESPROC image_rb;

static int side_open(struct side *s, const char *node)
{
    static const EGLint attrs[] = { EGL_CONTEXT_MAJOR_VERSION, 2, EGL_NONE };
    s->fd = open(node, O_RDWR | O_CLOEXEC);
    if (s->fd < 0)
        return -1;
    s->gbm = gbm_create_device(s->fd);
    if (!s->gbm)
        return -1;
    s->dpy = eglGetPlatformDisplay(EGL_PLATFORM_GBM_KHR, s->gbm, NULL);
    if (s->dpy == EGL_NO_DISPLAY || !eglInitialize(s->dpy, NULL, NULL))
        return -1;
    s->ctx = eglCreateContext(s->dpy, EGL_NO_CONFIG_KHR, EGL_NO_CONTEXT, attrs);
    return s->ctx == EGL_NO_CONTEXT ? -1 : 0;
}

static void side_close(struct side *s)
{
    eglMakeCurrent(s->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroyContext(s->dpy, s->ctx);
    eglTerminate(s->dpy);
    gbm_device_destroy(s->gbm);
    close(s->fd);
}

#define SHARE_W 256
#define SHARE_H 256

/* The four quadrant centres read back, as RGBA hex. */
static void read_quads(char *out)
{
    static const int at[4][2] = { { 64, 64 }, { 192, 64 }, { 64, 192 }, { 192, 192 } };
    for (int i = 0; i < 4; i++) {
        unsigned char px[4] = { 0 };
        glReadPixels(at[i][0], at[i][1], 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, px);
        sprintf(out + i * 9, "%02x%02x%02x%02x%c", px[0], px[1], px[2], px[3], i < 3 ? '/' : 0);
    }
}

/* The dma-buf of `bo` as a renderbuffer of the current context, bound to a
 * new framebuffer; 0 when the import fails. */
static GLuint import_fb(EGLDisplay dpy, struct gbm_bo *bo, uint64_t mod, EGLImageKHR *img, GLuint *rb)
{
    static const EGLint fd_k[4] = { EGL_DMA_BUF_PLANE0_FD_EXT, EGL_DMA_BUF_PLANE1_FD_EXT,
                                    EGL_DMA_BUF_PLANE2_FD_EXT, EGL_DMA_BUF_PLANE3_FD_EXT };
    static const EGLint off_k[4] = { EGL_DMA_BUF_PLANE0_OFFSET_EXT, EGL_DMA_BUF_PLANE1_OFFSET_EXT,
                                     EGL_DMA_BUF_PLANE2_OFFSET_EXT, EGL_DMA_BUF_PLANE3_OFFSET_EXT };
    static const EGLint pitch_k[4] = { EGL_DMA_BUF_PLANE0_PITCH_EXT, EGL_DMA_BUF_PLANE1_PITCH_EXT,
                                       EGL_DMA_BUF_PLANE2_PITCH_EXT, EGL_DMA_BUF_PLANE3_PITCH_EXT };
    static const EGLint lo_k[4] = { EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT,
                                    EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT, EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT };
    static const EGLint hi_k[4] = { EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT,
                                    EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT, EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT };
    EGLint a[64];
    int n = 0, fds[4] = { -1, -1, -1, -1 };
    int planes = gbm_bo_get_plane_count(bo);
    a[n++] = EGL_WIDTH; a[n++] = SHARE_W;
    a[n++] = EGL_HEIGHT; a[n++] = SHARE_H;
    a[n++] = EGL_LINUX_DRM_FOURCC_EXT; a[n++] = DRM_FORMAT_ARGB8888;
    for (int i = 0; i < planes && i < 4; i++) {
        fds[i] = gbm_bo_get_fd_for_plane(bo, i);
        a[n++] = fd_k[i]; a[n++] = fds[i];
        a[n++] = off_k[i]; a[n++] = (EGLint)gbm_bo_get_offset(bo, i);
        a[n++] = pitch_k[i]; a[n++] = (EGLint)gbm_bo_get_stride_for_plane(bo, i);
        a[n++] = lo_k[i]; a[n++] = (EGLint)(mod & 0xffffffff);
        a[n++] = hi_k[i]; a[n++] = (EGLint)(mod >> 32);
    }
    a[n++] = EGL_NONE;
    *img = create_image(dpy, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, NULL, a);
    for (int i = 0; i < 4; i++)
        if (fds[i] >= 0)
            close(fds[i]);
    if (*img == EGL_NO_IMAGE_KHR)
        return 0;
    GLuint fb;
    glGenRenderbuffers(1, rb);
    glBindRenderbuffer(GL_RENDERBUFFER, *rb);
    image_rb(GL_RENDERBUFFER, *img);
    glGenFramebuffers(1, &fb);
    glBindFramebuffer(GL_FRAMEBUFFER, fb);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, *rb);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) {
        glDeleteFramebuffers(1, &fb);
        return 0;
    }
    return fb;
}

static void import_drop(EGLDisplay dpy, GLuint fb, GLuint rb, EGLImageKHR img)
{
    if (fb)
        glDeleteFramebuffers(1, &fb);
    if (rb)
        glDeleteRenderbuffers(1, &rb);
    if (img != EGL_NO_IMAGE_KHR)
        destroy_image(dpy, img);
}

/* Full clear (a fast clear where the layout has one), then three quadrants
 * drawn over in A; read in A, then in B through the dma-buf. */
static void share_test(struct side *a, struct side *b, const char *name, uint64_t mod)
{
    static const char want[] = "204060ff/ff0000ff/00ff00ff/0000ffff";
    struct gbm_bo *bo = gbm_bo_create_with_modifiers2(a->gbm, SHARE_W, SHARE_H, DRM_FORMAT_ARGB8888, &mod, 1,
                                                      GBM_BO_USE_RENDERING);
    if (!bo) {
        printf("<0>[gpu] guest share %s: not offered\n", name);
        return;
    }
    char ra[40] = "-", rb_[40] = "-";
    EGLImageKHR img = EGL_NO_IMAGE_KHR;
    GLuint rb = 0;
    printf("<0>[gpu] guest share %s: drawing in A\n", name);
    eglMakeCurrent(a->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, a->ctx);
    GLuint fb = import_fb(a->dpy, bo, mod, &img, &rb);
    if (fb) {
        glDisable(GL_SCISSOR_TEST);
        glClearColor(0x20 / 255.0f, 0x40 / 255.0f, 0x60 / 255.0f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);
        glEnable(GL_SCISSOR_TEST);
        static const int q[3][2] = { { 128, 0 }, { 0, 128 }, { 128, 128 } };
        static const float c[3][3] = { { 1, 0, 0 }, { 0, 1, 0 }, { 0, 0, 1 } };
        for (int i = 0; i < 3; i++) {
            glScissor(q[i][0], q[i][1], 128, 128);
            glClearColor(c[i][0], c[i][1], c[i][2], 1.0f);
            glClear(GL_COLOR_BUFFER_BIT);
        }
        glDisable(GL_SCISSOR_TEST);
        glFinish();
        printf("<0>[gpu] guest share %s: drawn, reading in A\n", name);
        read_quads(ra);
    }
    import_drop(a->dpy, fb, rb, img);
    printf("<0>[gpu] guest share %s: A read %s, reading in B\n", name, ra);
    eglMakeCurrent(b->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, b->ctx);
    img = EGL_NO_IMAGE_KHR;
    rb = 0;
    fb = import_fb(b->dpy, bo, mod, &img, &rb);
    if (fb)
        read_quads(rb_);
    import_drop(b->dpy, fb, rb, img);
    eglMakeCurrent(b->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    printf("<0>[gpu] guest share %s (%d planes): A %s, B %s -> %s\n", name, gbm_bo_get_plane_count(bo), ra, rb_,
           strcmp(ra, want) ? "A WRONG" : strcmp(rb_, want) ? "B WRONG" : "ok");
    gbm_bo_destroy(bo);
}

/* `npkgpushare` as a word of /proc/cmdline. */
static int wants_share_tests(void)
{
    char line[4096];
    FILE *f = fopen("/proc/cmdline", "r");
    if (!f)
        return 0;
    size_t n = fread(line, 1, sizeof(line) - 1, f);
    fclose(f);
    line[n] = 0;
    for (char *w = strtok(line, " \n"); w; w = strtok(NULL, " \n"))
        if (!strcmp(w, "npkgpushare"))
            return 1;
    return 0;
}

static void share_tests(const char *node)
{
    struct side a, b;
    create_image = (PFNEGLCREATEIMAGEKHRPROC)eglGetProcAddress("eglCreateImageKHR");
    destroy_image = (PFNEGLDESTROYIMAGEKHRPROC)eglGetProcAddress("eglDestroyImageKHR");
    image_rb = (PFNGLEGLIMAGETARGETRENDERBUFFERSTORAGEOESPROC)eglGetProcAddress("glEGLImageTargetRenderbufferStorageOES");
    if (!create_image || !destroy_image || !image_rb || side_open(&a, node) || side_open(&b, node)) {
        printf("<0>[gpu] guest share: setup failed (egl 0x%x)\n", eglGetError());
        return;
    }
    share_test(&a, &b, "linear", DRM_FORMAT_MOD_LINEAR);
    share_test(&a, &b, "X", I915_FORMAT_MOD_X_TILED);
    share_test(&a, &b, "Y", I915_FORMAT_MOD_Y_TILED);
    share_test(&a, &b, "Y+RC_CCS_CC", I915_FORMAT_MOD_Y_TILED_GEN12_RC_CCS_CC);
    share_test(&a, &b, "Y+RC_CCS", I915_FORMAT_MOD_Y_TILED_GEN12_RC_CCS);
    side_close(&a);
    side_close(&b);
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
    if (wants_share_tests())
        share_tests(node);
    return 0;
}
