// Core Animation transports the presented surface between processes. The Rust
// owners call these functions on their respective AppKit main threads.
#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>
#import <QuartzCore/CARemoteLayerServer.h>
#import <QuartzCore/CARemoteLayerClient.h>
#import <servers/bootstrap.h>

int vk_register_layer_server(const char *name) {
    mach_port_t port = CARemoteLayerServer.sharedServer.serverPort;
    if (!MACH_PORT_VALID(port)) return KERN_FAILURE;
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    return bootstrap_register(bootstrap_port, (char *)name, port);
}
void vk_unregister_layer_server(const char *name) {
    (void)bootstrap_register(bootstrap_port, (char *)name, MACH_PORT_NULL);
#pragma clang diagnostic pop
}

void *vk_export_layer(const char *name, void *view_ptr, uint32_t *identifier) {
    mach_port_t port = MACH_PORT_NULL;
    if (bootstrap_look_up(bootstrap_port, (char *)name, &port) != KERN_SUCCESS) return NULL;
    CARemoteLayerClient *client = [[CARemoteLayerClient alloc] initWithServerPort:port];
    mach_port_deallocate(mach_task_self(), port);
    NSView *view = (__bridge NSView *)view_ptr;
    if (!client || !view.layer) return NULL;
    // Surfman owns the view's layer and swaps its IOSurface at presentation.
    client.layer = view.layer;
    *identifier = client.clientId;
    return (__bridge_retained void *)client;
}
void vk_end_export(void *ptr) {
    CARemoteLayerClient *client = CFBridgingRelease(ptr);
    [client invalidate];
}

void *vk_import_layer(uint32_t identifier) {
    CALayer *layer = [CALayer layerWithRemoteClientId:identifier];
    layer.masksToBounds = YES;
    layer.anchorPoint = CGPointZero;
    return (__bridge_retained void *)layer;
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
        // CALayer geometry is expressed in its parent's coordinate system.
        layer.frame = CGRectMake(x, parent.geometryFlipped ? y : parent.bounds.size.height - y - h, w, h);
    }
    [CATransaction commit];
}
void vk_end_import(void *ptr) {
    CALayer *layer = CFBridgingRelease(ptr);
    [layer removeFromSuperlayer];
}
