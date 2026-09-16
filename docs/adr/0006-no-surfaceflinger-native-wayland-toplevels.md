# No SurfaceFlinger equivalent, apps get native Wayland toplevels

Waydroid composites all app surfaces through SurfaceFlinger inside the container
and bridges the result to Wayland through a custom hwcomposer. In Mosaic each
app's reimplemented `Surface` and `ViewRootImpl` creates its own native Wayland
`xdg_toplevel` and draws to it directly via EGL or Vulkan; the host compositor
performs all frame compositing. The broker only arbitrates Android-specific
window policy and never composites pixels. This is a large scope cut against
reimplementing SurfaceFlinger's buffer pipeline, and it is what native execution
means for windowing.
