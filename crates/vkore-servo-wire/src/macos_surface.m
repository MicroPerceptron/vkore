// The helper publishes a GPU-only snapshot. Only the receiver of the Mach
// send right can look up the IOSurface; no global surface ID is exposed.
#define GL_SILENCE_DEPRECATION
#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>
#import <IOSurface/IOSurface.h>
#import <OpenGL/OpenGL.h>
#import <OpenGL/CGLIOSurface.h>
#import <OpenGL/gl3.h>
#import <servers/bootstrap.h>

static const mach_msg_id_t frame_message = 1;

#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
mach_port_t vk_register_layer_server(const char *name) {
    mach_port_t port = MACH_PORT_NULL;
    if (mach_port_allocate(mach_task_self(), MACH_PORT_RIGHT_RECEIVE, &port) != KERN_SUCCESS)
        return MACH_PORT_NULL;
    if (mach_port_insert_right(mach_task_self(), port, port, MACH_MSG_TYPE_MAKE_SEND) != KERN_SUCCESS) {
        mach_port_mod_refs(mach_task_self(), port, MACH_PORT_RIGHT_RECEIVE, -1);
        return MACH_PORT_NULL;
    }
    int status = bootstrap_register(bootstrap_port, (char *)name, port);
    mach_port_deallocate(mach_task_self(), port);
    if (status != KERN_SUCCESS) {
        mach_port_mod_refs(mach_task_self(), port, MACH_PORT_RIGHT_RECEIVE, -1);
        return MACH_PORT_NULL;
    }
    return port;
}
void vk_unregister_layer_server(const char *name) {
    (void)bootstrap_register(bootstrap_port, (char *)name, MACH_PORT_NULL);
}
#pragma clang diagnostic pop

void vk_destroy_receiver(mach_port_t port) {
    mach_port_mod_refs(mach_task_self(), port, MACH_PORT_RIGHT_RECEIVE, -1);
}
mach_port_t vk_connect_export(const char *name) {
    mach_port_t port = MACH_PORT_NULL;
    if (bootstrap_look_up(bootstrap_port, (char *)name, &port) != KERN_SUCCESS)
        return MACH_PORT_NULL;
    return port;
}
void vk_end_export(mach_port_t port) { mach_port_deallocate(mach_task_self(), port); }

// Copy the current Servo read framebuffer directly to an immutable IOSurface.
// Surfman's window swap chain can reuse its two textures before a different
// process's layer has finished displaying one of them.
static bool copy_frame(IOSurfaceRef surface, GLsizei w, GLsizei h) {
    CGLContextObj context = CGLGetCurrentContext();
    if (!context) return false;
    GLint previous_texture, previous_draw;
    glGetIntegerv(GL_TEXTURE_BINDING_RECTANGLE, &previous_texture);
    glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &previous_draw);
    GLboolean scissor = glIsEnabled(GL_SCISSOR_TEST);
    GLuint texture, fbo;
    glGenTextures(1, &texture);
    glGenFramebuffers(1, &fbo);
    glBindTexture(GL_TEXTURE_RECTANGLE, texture);
    bool ok = CGLTexImageIOSurface2D(context, GL_TEXTURE_RECTANGLE, GL_RGBA8, w, h,
                                     GL_BGRA, GL_UNSIGNED_INT_8_8_8_8_REV, surface, 0) == kCGLNoError;
    if (ok) {
        glBindFramebuffer(GL_DRAW_FRAMEBUFFER, fbo);
        glFramebufferTexture2D(GL_DRAW_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_RECTANGLE, texture, 0);
        ok = glCheckFramebufferStatus(GL_DRAW_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE;
        if (ok) {
            glDisable(GL_SCISSOR_TEST);
            // GL's first row is the bottom row; Core Animation displays it at the top.
            glBlitFramebuffer(0, 0, w, h, 0, h, w, 0, GL_COLOR_BUFFER_BIT, GL_NEAREST);
            glFinish();
            ok = glGetError() == GL_NO_ERROR;
            if (scissor) glEnable(GL_SCISSOR_TEST);
        }
    }
    glBindFramebuffer(GL_DRAW_FRAMEBUFFER, previous_draw);
    glBindTexture(GL_TEXTURE_RECTANGLE, previous_texture);
    glDeleteFramebuffers(1, &fbo);
    glDeleteTextures(1, &texture);
    return ok;
}

int vk_send_frame(mach_port_t destination, uint32_t w, uint32_t h) {
    NSDictionary *props = @{(id)kIOSurfaceWidth: @(w), (id)kIOSurfaceHeight: @(h),
                            (id)kIOSurfaceBytesPerElement: @4, (id)kIOSurfacePixelFormat: @((uint32_t)'BGRA')};
    IOSurfaceRef surface = IOSurfaceCreate((__bridge CFDictionaryRef)props);
    if (!surface) return KERN_RESOURCE_SHORTAGE;
    mach_port_t right = copy_frame(surface, (GLsizei)w, (GLsizei)h)
        ? IOSurfaceCreateMachPort(surface) : MACH_PORT_NULL;
    CFRelease(surface);
    if (!MACH_PORT_VALID(right)) return KERN_FAILURE;
    mach_msg_header_t msg = {
        .msgh_bits = MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_COPY_SEND),
        .msgh_size = sizeof(msg), .msgh_remote_port = destination,
        .msgh_local_port = right, .msgh_id = frame_message,
    };
    int result = mach_msg(&msg, MACH_SEND_MSG | MACH_SEND_TIMEOUT, sizeof(msg), 0,
                          MACH_PORT_NULL, 0, MACH_PORT_NULL);
    mach_port_deallocate(mach_task_self(), right);
    return result;
}

void *vk_import_layer(void) {
    CALayer *layer = [CALayer layer];
    layer.masksToBounds = YES;
    layer.zPosition = 1;
    layer.contentsGravity = kCAGravityResize;
    return (__bridge_retained void *)layer;
}
int vk_receive_frame(mach_port_t receiver, void *ptr, uint32_t max_w, uint32_t max_h, uint64_t max_pixels) {
    struct { mach_msg_header_t header; mach_msg_max_trailer_t trailer; } msg = {0};
    int result = mach_msg(&msg.header, MACH_RCV_MSG | MACH_RCV_TIMEOUT, 0, sizeof(msg),
                          receiver, 0, MACH_PORT_NULL);
    if (result != MACH_MSG_SUCCESS) return result;
    IOSurfaceRef surface = NULL;
    if (msg.header.msgh_size == sizeof(msg.header) && msg.header.msgh_id == frame_message &&
        !(msg.header.msgh_bits & MACH_MSGH_BITS_COMPLEX) &&
        MACH_MSGH_BITS_REMOTE(msg.header.msgh_bits) == MACH_MSG_TYPE_PORT_SEND)
        surface = IOSurfaceLookupFromMachPort(msg.header.msgh_remote_port);
    mach_msg_destroy(&msg.header);
    if (!surface) return KERN_INVALID_ARGUMENT;
    size_t w = IOSurfaceGetWidth(surface), h = IOSurfaceGetHeight(surface);
    if (!w || !h || w > max_w || h > max_h || w * h > max_pixels ||
        IOSurfaceGetPixelFormat(surface) != (uint32_t)'BGRA') {
        CFRelease(surface);
        return KERN_INVALID_ARGUMENT;
    }
    CALayer *layer = (__bridge CALayer *)ptr;
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    layer.contents = (__bridge id)surface;
    [CATransaction commit];
    CFRelease(surface);
    return KERN_SUCCESS;
}
void vk_place_layer(void *ptr, void *view_ptr, double x, double y, double w, double h) {
    CALayer *layer = (__bridge CALayer *)ptr;
    NSView *view = (__bridge NSView *)view_ptr;
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    if (w <= 0 || h <= 0) {
        [layer removeFromSuperlayer];
    } else {
        view.wantsLayer = YES;
        CALayer *parent = view.layer;
        if (layer.superlayer != parent) [parent addSublayer:layer];
        layer.frame = CGRectMake(x, parent.geometryFlipped ? y : parent.bounds.size.height - y - h, w, h);
    }
    [CATransaction commit];
}
void vk_end_import(void *ptr) {
    CALayer *layer = CFBridgingRelease(ptr);
    [layer removeFromSuperlayer];
}
