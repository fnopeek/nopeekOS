/* npk-gpuprobe: open the virtio-gpu render node the way an EGL client
 * does without a compositor (GBM platform), make a GLES2 context current,
 * print the driver Mesa ended up on, then clear a small renderbuffer to a
 * known colour and read it back: that runs a batch on the card and shows
 * whether its result reaches memory, and a depth-tested draw (the test
 * every 3D client relies on; on Intel it goes through HiZ). Run once per
 * guest session when the host offers CONTEXT_INIT; "llvmpipe" means the
 * native context did not come up, and the host log says why.
 *
 * With `npkgpushare` on the kernel command line, then the path a Wayland
 * client and its compositor take: for each layout (linear, Y, Y with
 * render compression and its clear colour) one buffer is made and drawn
 * in a first context the way a window surface is, read back there, then
 * imported through its dma-buf into a second context on its own fd and
 * read there as a renderbuffer, sampled as a texture the way the
 * compositor composes it, and imported without its modifier. Then what a
 * browser does within its own context: a large texture uploaded by the CPU
 * and sampled, and a texture rendered to and sampled. A line before each
 * step names the step, so a hang shows where it stopped. */
#define _GNU_SOURCE
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <fcntl.h>
#include <drm_fourcc.h>
#include <gbm.h>
#include <stdint.h>
#include <stdlib.h>
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
static PFNGLEGLIMAGETARGETTEXTURE2DOESPROC image_tex;

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

/* The dma-buf of `bo` as an image of `dpy`, with its modifier or, as an
 * implicit-modifier client sends it, without. */
static EGLImageKHR import_image(EGLDisplay dpy, struct gbm_bo *bo, uint64_t mod, int with_mod)
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
    int planes = with_mod ? gbm_bo_get_plane_count(bo) : 1;
    a[n++] = EGL_WIDTH; a[n++] = SHARE_W;
    a[n++] = EGL_HEIGHT; a[n++] = SHARE_H;
    a[n++] = EGL_LINUX_DRM_FOURCC_EXT; a[n++] = DRM_FORMAT_ARGB8888;
    for (int i = 0; i < planes && i < 4; i++) {
        fds[i] = gbm_bo_get_fd_for_plane(bo, i);
        a[n++] = fd_k[i]; a[n++] = fds[i];
        a[n++] = off_k[i]; a[n++] = (EGLint)gbm_bo_get_offset(bo, i);
        a[n++] = pitch_k[i]; a[n++] = (EGLint)gbm_bo_get_stride_for_plane(bo, i);
        if (with_mod) {
            a[n++] = lo_k[i]; a[n++] = (EGLint)(mod & 0xffffffff);
            a[n++] = hi_k[i]; a[n++] = (EGLint)(mod >> 32);
        }
    }
    a[n++] = EGL_NONE;
    EGLImageKHR img = create_image(dpy, EGL_NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, NULL, a);
    for (int i = 0; i < 4; i++)
        if (fds[i] >= 0)
            close(fds[i]);
    return img;
}

/* `img` as the colour buffer of a new framebuffer of the current context;
 * 0 when incomplete. */
static GLuint image_fb(EGLImageKHR img, GLuint *rb)
{
    GLuint fb;
    glGenRenderbuffers(1, rb);
    glBindRenderbuffer(GL_RENDERBUFFER, *rb);
    image_rb(GL_RENDERBUFFER, img);
    glGenFramebuffers(1, &fb);
    glBindFramebuffer(GL_FRAMEBUFFER, fb);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, *rb);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) {
        glDeleteFramebuffers(1, &fb);
        return 0;
    }
    return fb;
}

/* `img` sampled as a texture over a full-screen quad into a plain
 * renderbuffer of the current context, the way a compositor composes a
 * client buffer; the quadrants of the result read back into `out`. */
static void sample_read(EGLImageKHR img, char *out)
{
    static const char *vs = "attribute vec2 p; varying vec2 t;"
                            "void main() { t = p * 0.5 + 0.5; gl_Position = vec4(p, 0.0, 1.0); }";
    static const char *fs = "#extension GL_OES_EGL_image_external : require\n"
                            "precision mediump float; varying vec2 t; uniform samplerExternalOES s;"
                            "void main() { gl_FragColor = texture2D(s, t); }";
    static const GLfloat quad[] = { -1, -1, 1, -1, -1, 1, 1, 1 };
    GLuint sh[2] = { glCreateShader(GL_VERTEX_SHADER), glCreateShader(GL_FRAGMENT_SHADER) };
    glShaderSource(sh[0], 1, &vs, NULL);
    glShaderSource(sh[1], 1, &fs, NULL);
    glCompileShader(sh[0]);
    glCompileShader(sh[1]);
    GLuint prog = glCreateProgram();
    glAttachShader(prog, sh[0]);
    glAttachShader(prog, sh[1]);
    glBindAttribLocation(prog, 0, "p");
    glLinkProgram(prog);
    GLint linked = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &linked);
    GLuint tex, rb, fb;
    glGenTextures(1, &tex);
    glBindTexture(GL_TEXTURE_EXTERNAL_OES, tex);
    glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_EXTERNAL_OES, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    image_tex(GL_TEXTURE_EXTERNAL_OES, img);
    glGenRenderbuffers(1, &rb);
    glBindRenderbuffer(GL_RENDERBUFFER, rb);
    glRenderbufferStorage(GL_RENDERBUFFER, GL_RGBA8_OES, SHARE_W, SHARE_H);
    glGenFramebuffers(1, &fb);
    glBindFramebuffer(GL_FRAMEBUFFER, fb);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, rb);
    if (linked && glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE) {
        glViewport(0, 0, SHARE_W, SHARE_H);
        glUseProgram(prog);
        glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, quad);
        glEnableVertexAttribArray(0);
        glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
        glFinish();
        read_quads(out);
    } else {
        strcpy(out, linked ? "fb incomplete" : "no program");
    }
    glDeleteFramebuffers(1, &fb);
    glDeleteRenderbuffers(1, &rb);
    glDeleteTextures(1, &tex);
    glDeleteProgram(prog);
    glDeleteShader(sh[0]);
    glDeleteShader(sh[1]);
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

/* The buffer made the way a client's window surface is (GBM, the driver's
 * own allocation) and drawn in A: a full clear (a fast clear where the
 * layout has one), then three quadrants. Read back in A, then in B through
 * the dma-buf three ways: as a renderbuffer, sampled as a texture the way
 * the compositor does, and imported without the modifier (the driver then
 * asks the kernel for the tiling). */
static void share_test(struct side *a, struct side *b, const char *name, uint64_t mod)
{
    static const char want[] = "204060ff/ff0000ff/00ff00ff/0000ffff";
    struct gbm_bo *bo = gbm_bo_create_with_modifiers2(a->gbm, SHARE_W, SHARE_H, DRM_FORMAT_ARGB8888, &mod, 1,
                                                      GBM_BO_USE_RENDERING);
    if (!bo) {
        printf("<0>[gpu] guest share %s: not offered\n", name);
        return;
    }
    char ra[40] = "-", rb_[40] = "-", rt[40] = "-", ri[40] = "-";
    GLuint rb = 0, fb;
    printf("<0>[gpu] guest share %s: drawing in A\n", name);
    eglMakeCurrent(a->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, a->ctx);
    EGLImageKHR img = create_image(a->dpy, EGL_NO_CONTEXT, EGL_NATIVE_PIXMAP_KHR, (EGLClientBuffer)bo, NULL);
    fb = img != EGL_NO_IMAGE_KHR ? image_fb(img, &rb) : 0;
    if (fb) {
        glViewport(0, 0, SHARE_W, SHARE_H);
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
        glFlush();
        glFinish();
        read_quads(ra);
    }
    import_drop(a->dpy, fb, rb, img);
    printf("<0>[gpu] guest share %s: A read %s, reading in B\n", name, ra);

    eglMakeCurrent(b->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, b->ctx);
    img = import_image(b->dpy, bo, mod, 1);
    rb = 0;
    fb = img != EGL_NO_IMAGE_KHR ? image_fb(img, &rb) : 0;
    if (fb)
        read_quads(rb_);
    if (img != EGL_NO_IMAGE_KHR)
        sample_read(img, rt);
    import_drop(b->dpy, fb, rb, img);
    if (gbm_bo_get_plane_count(bo) == 1) {
        img = import_image(b->dpy, bo, mod, 0);
        rb = 0;
        fb = img != EGL_NO_IMAGE_KHR ? image_fb(img, &rb) : 0;
        if (fb)
            read_quads(ri);
        import_drop(b->dpy, fb, rb, img);
    }
    eglMakeCurrent(b->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    printf("<0>[gpu] guest share %s (%d planes): A %s | B read %s %s | B sampled %s %s | B implicit %s %s\n",
           name, gbm_bo_get_plane_count(bo), strcmp(ra, want) ? ra : "ok",
           strcmp(rb_, want) ? "WRONG" : "ok", strcmp(rb_, want) ? rb_ : "",
           strcmp(rt, want) ? "WRONG" : "ok", strcmp(rt, want) ? rt : "",
           ri[0] == '-' ? "-" : strcmp(ri, want) ? "WRONG" : "ok", strcmp(ri, want) ? ri : "");
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

/* A program drawing texture unit 0 (sampler2D) over a full-screen quad. */
static GLuint tex_program(void)
{
    static const char *vs = "attribute vec2 p; varying vec2 t;"
                            "void main() { t = p * 0.5 + 0.5; gl_Position = vec4(p, 0.0, 1.0); }";
    static const char *fs = "precision mediump float; varying vec2 t; uniform sampler2D s;"
                            "void main() { gl_FragColor = texture2D(s, t); }";
    GLuint sh[2] = { glCreateShader(GL_VERTEX_SHADER), glCreateShader(GL_FRAGMENT_SHADER) };
    glShaderSource(sh[0], 1, &vs, NULL);
    glShaderSource(sh[1], 1, &fs, NULL);
    glCompileShader(sh[0]);
    glCompileShader(sh[1]);
    GLuint prog = glCreateProgram();
    glAttachShader(prog, sh[0]);
    glAttachShader(prog, sh[1]);
    glBindAttribLocation(prog, 0, "p");
    glLinkProgram(prog);
    glDeleteShader(sh[0]);
    glDeleteShader(sh[1]);
    GLint ok = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &ok);
    return ok ? prog : 0;
}

/* `tex` drawn into a new w x h renderbuffer; that framebuffer stays bound
 * for reading. Returns the framebuffer, 0 when incomplete. */
static GLuint draw_tex(GLuint prog, GLuint tex, int w, int h, GLuint *rb)
{
    static const GLfloat quad[] = { -1, -1, 1, -1, -1, 1, 1, 1 };
    GLuint fb;
    glGenRenderbuffers(1, rb);
    glBindRenderbuffer(GL_RENDERBUFFER, *rb);
    glRenderbufferStorage(GL_RENDERBUFFER, GL_RGBA8_OES, w, h);
    glGenFramebuffers(1, &fb);
    glBindFramebuffer(GL_FRAMEBUFFER, fb);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, *rb);
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) {
        glDeleteFramebuffers(1, &fb);
        return 0;
    }
    glViewport(0, 0, w, h);
    glUseProgram(prog);
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, tex);
    glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, quad);
    glEnableVertexAttribArray(0);
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    glFinish();
    return fb;
}

#define UP 2048

static uint32_t up_pixel(int x, int y)
{
    return (uint32_t)(x & 255) | (uint32_t)(y & 255) << 8 | (uint32_t)((x >> 8) | (y >> 8) << 4) << 16 | 0xffu << 24;
}

/* What a browser does with glyphs and images: a large texture written by
 * the CPU (glTexImage2D, staged through a buffer the CPU maps), then
 * sampled on the card; 16 points spread over it must hold the pattern.
 * Then render to texture: quadrants cleared into a texture, sampled into
 * another target, read back. */
static void upload_tests(struct side *a)
{
    eglMakeCurrent(a->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, a->ctx);
    GLuint prog = tex_program();
    if (!prog) {
        printf("<0>[gpu] guest upload: no program\n");
        return;
    }
    uint32_t *img = malloc((size_t)UP * UP * 4);
    if (!img)
        return;
    for (int y = 0; y < UP; y++)
        for (int x = 0; x < UP; x++)
            img[(size_t)y * UP + x] = up_pixel(x, y);
    GLuint tex, rb;
    glGenTextures(1, &tex);
    glBindTexture(GL_TEXTURE_2D, tex);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
    glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, UP, UP, 0, GL_RGBA, GL_UNSIGNED_BYTE, img);
    free(img);
    GLuint fb = draw_tex(prog, tex, UP, UP, &rb);
    int bad = 0, first_x = -1, first_y = -1;
    uint32_t got_first = 0;
    for (int i = 0; fb && i < 16; i++) {
        int x = (i * 523 + 17) % UP, y = (i * 977 + 31) % UP;
        uint32_t px = 0;
        glReadPixels(x, y, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, &px);
        if (px != up_pixel(x, y) && bad++ == 0) {
            first_x = x;
            first_y = y;
            got_first = px;
        }
    }
    if (!fb)
        printf("<0>[gpu] guest upload %dx%d: framebuffer incomplete\n", UP, UP);
    else if (bad)
        printf("<0>[gpu] guest upload %dx%d: %d of 16 WRONG, first at %d,%d: %08x want %08x\n", UP, UP, bad,
               first_x, first_y, got_first, up_pixel(first_x, first_y));
    else
        printf("<0>[gpu] guest upload %dx%d: 16 of 16 ok\n", UP, UP);
    if (fb)
        glDeleteFramebuffers(1, &fb);
    glDeleteRenderbuffers(1, &rb);

    /* Render to texture. */
    GLuint rt, rtfb;
    glBindTexture(GL_TEXTURE_2D, tex);
    glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, 1024, 1024, 0, GL_RGBA, GL_UNSIGNED_BYTE, NULL);
    rt = tex;
    glGenFramebuffers(1, &rtfb);
    glBindFramebuffer(GL_FRAMEBUFFER, rtfb);
    glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, rt, 0);
    char out[40] = "fb incomplete";
    if (glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE) {
        glViewport(0, 0, 1024, 1024);
        glDisable(GL_SCISSOR_TEST);
        glClearColor(0x20 / 255.0f, 0x40 / 255.0f, 0x60 / 255.0f, 1.0f);
        glClear(GL_COLOR_BUFFER_BIT);
        glEnable(GL_SCISSOR_TEST);
        static const int q[3][2] = { { 512, 0 }, { 0, 512 }, { 512, 512 } };
        static const float c[3][3] = { { 1, 0, 0 }, { 0, 1, 0 }, { 0, 0, 1 } };
        for (int i = 0; i < 3; i++) {
            glScissor(q[i][0], q[i][1], 512, 512);
            glClearColor(c[i][0], c[i][1], c[i][2], 1.0f);
            glClear(GL_COLOR_BUFFER_BIT);
        }
        glDisable(GL_SCISSOR_TEST);
        GLuint fb2 = draw_tex(prog, rt, 1024, 1024, &rb);
        if (fb2) {
            static const int at[4][2] = { { 256, 256 }, { 768, 256 }, { 256, 768 }, { 768, 768 } };
            for (int i = 0; i < 4; i++) {
                unsigned char px[4] = { 0 };
                glReadPixels(at[i][0], at[i][1], 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, px);
                sprintf(out + i * 9, "%02x%02x%02x%02x%c", px[0], px[1], px[2], px[3], i < 3 ? '/' : 0);
            }
            glDeleteFramebuffers(1, &fb2);
        }
        glDeleteRenderbuffers(1, &rb);
    }
    printf("<0>[gpu] guest render to texture: %s -> %s\n", out,
           strcmp(out, "204060ff/ff0000ff/00ff00ff/0000ffff") ? "WRONG" : "ok");
    glDeleteFramebuffers(1, &rtfb);
    glDeleteTextures(1, &tex);
    glDeleteProgram(prog);
    eglMakeCurrent(a->dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
}

static void share_tests(const char *node)
{
    struct side a, b;
    create_image = (PFNEGLCREATEIMAGEKHRPROC)eglGetProcAddress("eglCreateImageKHR");
    destroy_image = (PFNEGLDESTROYIMAGEKHRPROC)eglGetProcAddress("eglDestroyImageKHR");
    image_rb = (PFNGLEGLIMAGETARGETRENDERBUFFERSTORAGEOESPROC)eglGetProcAddress("glEGLImageTargetRenderbufferStorageOES");
    image_tex = (PFNGLEGLIMAGETARGETTEXTURE2DOESPROC)eglGetProcAddress("glEGLImageTargetTexture2DOES");
    if (!create_image || !destroy_image || !image_rb || !image_tex || side_open(&a, node) || side_open(&b, node)) {
        printf("<0>[gpu] guest share: setup failed (egl 0x%x)\n", eglGetError());
        return;
    }
    share_test(&a, &b, "linear", DRM_FORMAT_MOD_LINEAR);
    share_test(&a, &b, "Y", I915_FORMAT_MOD_Y_TILED);
    share_test(&a, &b, "Y+RC_CCS_CC", I915_FORMAT_MOD_Y_TILED_GEN12_RC_CCS_CC);
    upload_tests(&a);
    side_close(&a);
    side_close(&b);
}

/* A program drawing a full-screen quad at depth `z` in colour `c`. */
static GLuint flat_program(GLint *z, GLint *c)
{
    static const char *vs = "attribute vec2 p; uniform float z;"
                            "void main() { gl_Position = vec4(p, z, 1.0); }";
    static const char *fs = "precision mediump float; uniform vec4 c;"
                            "void main() { gl_FragColor = c; }";
    GLuint sh[2] = { glCreateShader(GL_VERTEX_SHADER), glCreateShader(GL_FRAGMENT_SHADER) };
    glShaderSource(sh[0], 1, &vs, NULL);
    glShaderSource(sh[1], 1, &fs, NULL);
    glCompileShader(sh[0]);
    glCompileShader(sh[1]);
    GLuint prog = glCreateProgram();
    glAttachShader(prog, sh[0]);
    glAttachShader(prog, sh[1]);
    glBindAttribLocation(prog, 0, "p");
    glLinkProgram(prog);
    glDeleteShader(sh[0]);
    glDeleteShader(sh[1]);
    GLint ok = 0;
    glGetProgramiv(prog, GL_LINK_STATUS, &ok);
    *z = glGetUniformLocation(prog, "z");
    *c = glGetUniformLocation(prog, "c");
    return ok ? prog : 0;
}

/* What every 3D client does: a depth buffer cleared to 1.0, GL_LESS. A
 * red quad at z 0 passes, a green one behind it at 0.5 is discarded, a blue
 * one in front at -0.5 passes: the centre must end blue. Three ways, each on
 * a fresh buffer, each frame twice:
 *   batches - glClear, glFinish, again (a frame loop);
 *   one     - both frames in one batch, read at the end;
 *   drawn   - the second frame resets depth by drawing at z 1 with
 *             GL_ALWAYS instead of glClear (no HiZ fast clear). */
static void depth_test(void)
{
    static const GLfloat quad[] = { -1, -1, 1, -1, -1, 1, 1, 1 };
    static const float zs[3] = { 0.0f, 0.5f, -0.5f };
    static const float cs[3][4] = { { 1, 0, 0, 1 }, { 0, 1, 0, 1 }, { 0, 0, 1, 1 } };
    static const char *names[3] = { "batches", "one", "drawn" };
    GLint uz, uc;
    GLuint prog = flat_program(&uz, &uc);
    char line[256];
    int n = snprintf(line, sizeof(line), "LESS, cleared 1.0:");
    int bad = 0;
    for (int mode = 0; mode < 3; mode++) {
        GLuint rb[2], fb;
        glGenRenderbuffers(2, rb);
        glBindRenderbuffer(GL_RENDERBUFFER, rb[0]);
        glRenderbufferStorage(GL_RENDERBUFFER, GL_RGBA8_OES, 256, 256);
        glBindRenderbuffer(GL_RENDERBUFFER, rb[1]);
        glRenderbufferStorage(GL_RENDERBUFFER, GL_DEPTH_COMPONENT16, 256, 256);
        glGenFramebuffers(1, &fb);
        glBindFramebuffer(GL_FRAMEBUFFER, fb);
        glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, rb[0]);
        glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_DEPTH_ATTACHMENT, GL_RENDERBUFFER, rb[1]);
        char out[2][16] = { "-", "-" };
        if (prog && glCheckFramebufferStatus(GL_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE) {
            glViewport(0, 0, 256, 256);
            glUseProgram(prog);
            glVertexAttribPointer(0, 2, GL_FLOAT, GL_FALSE, 0, quad);
            glEnableVertexAttribArray(0);
            glEnable(GL_DEPTH_TEST);
            for (int round = 0; round < 2; round++) {
                glClearColor(0, 0, 0, 1);
                if (mode == 2 && round == 1) {
                    glClear(GL_COLOR_BUFFER_BIT);
                    glDepthFunc(GL_ALWAYS);
                    glColorMask(GL_FALSE, GL_FALSE, GL_FALSE, GL_FALSE);
                    glUniform1f(uz, 1.0f);
                    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
                    glColorMask(GL_TRUE, GL_TRUE, GL_TRUE, GL_TRUE);
                } else {
                    glClearDepthf(1.0f);
                    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
                }
                glDepthFunc(GL_LESS);
                for (int i = 0; i < 3; i++) {
                    glUniform1f(uz, zs[i]);
                    glUniform4fv(uc, 1, cs[i]);
                    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
                }
                if (mode != 1 || round == 1) {
                    glFinish();
                    unsigned char px[4] = { 0 };
                    glReadPixels(128, 128, 1, 1, GL_RGBA, GL_UNSIGNED_BYTE, px);
                    sprintf(out[round], "%02x%02x%02x%02x", px[0], px[1], px[2], px[3]);
                }
            }
            glDisable(GL_DEPTH_TEST);
        }
        int ok = (mode == 1 || !strcmp(out[0], "0000ffff")) && !strcmp(out[1], "0000ffff");
        bad += !ok;
        n += snprintf(line + n, sizeof(line) - n, " %s %s/%s %s;", names[mode], out[0], out[1], ok ? "ok" : "WRONG");
        glDeleteFramebuffers(1, &fb);
        glDeleteRenderbuffers(2, rb);
    }
    printf("<0>[gpu] guest depth test (%s) -> %s\n", line, bad ? "WRONG" : "ok");
    if (prog)
        glDeleteProgram(prog);
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
    depth_test();

    eglMakeCurrent(dpy, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
    eglDestroyContext(dpy, ctx);
    eglTerminate(dpy);
    gbm_device_destroy(gbm);
    close(fd);
    if (wants_share_tests())
        share_tests(node);
    return 0;
}
