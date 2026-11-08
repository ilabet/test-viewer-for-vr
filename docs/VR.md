# VR headsets (OpenXR)

Aurora can draw the world in a PC VR headset through OpenXR: SteamVR (Vive,
Index, WMR, Pico), the Meta Quest runtime (Quest through Link or Air Link),
Virtual Desktop, or any other OpenXR runtime with Vulkan support
(`XR_KHR_vulkan_enable2`). Standalone Quest is not supported: the headset's
mobile GPU cannot run a desktop viewer.

Status: experimental, not yet tested on a headset.

## Building and starting

VR support is behind the `openxr` feature; the OpenXR loader is found at run
time, so the build needs nothing installed.

```sh
cargo run -p aurora-viewer --features openxr
```

Start your runtime (SteamVR, Meta Quest Link…), then start Aurora with
`AURORA_VR=1`. Without a runtime, without a headset, or without that
variable, Aurora starts on the desktop as usual; the log says why
(`VR: no OpenXR runtime found`, `VR: no headset connected`…).

macOS has no OpenXR runtime with Vulkan: the feature does nothing there.

## Controls (VRChat-style)

| Control | Action |
| --- | --- |
| Left stick | Walk relative to where the avatar faces; push fully to run |
| Right stick | Snap turn (30°) |
| A | Jump; double tap toggles flying; while flying, hold to rise |
| X | While flying, hold to descend |
| Grip | Stand up when seated |
| Moving or jumping while seated | Stand up |

Not wired yet: B/Y menu, trigger pointing and clicking in the world.

## How it works

- `aurora-xr` opens the runtime, creates the Vulkan instance and device
  through it (`XR_KHR_vulkan_enable2`) and wraps them for wgpu, then runs the
  session: stereo swapchain, head and eye poses, controller actions.
- `aurora-render` (`xr.rs`) uses that device for the whole renderer. Each eye
  is drawn with Aurora's normal frame, offscreen with the eye's view and
  projection, then copied into its layer of the headset's image. Frames are
  drawn on the main thread in VR so an image is handed back only once both
  eyes are submitted.
- `aurora-viewer` (`vr.rs`) places the VR floor at the avatar's feet, facing
  the avatar's direction, and maps the controllers onto the movement keys.
  The desktop window keeps its normal view.

## Known limits

- Each frame is drawn three times (two eyes and the window), at the window's
  size; the runtime scales the eye images to the display.
- Draw lists are culled for the window's camera, so objects outside its view
  can be missing in the headset. Mouselook (M) keeps them close.
- The interface (menus, chat) is shown on the desktop window only.
