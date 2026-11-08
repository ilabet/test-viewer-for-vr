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
| B / Y | Open or close the VR menu |
| Trigger | Touch the object the controller points at (as a left click); in the menu, select |
| Grip | Sit on the object the controller points at; stand up when seated |
| Moving or jumping while seated | Stand up |

The VR menu, shown in front of you and on the desktop window: Fly / Land,
Sit down / Stand up, Always run, Turning (snap or smooth), Close. The right
stick chooses (up/down) while it is open, the trigger selects.

### Trying it without a headset

`AURORA_VR_SIM=1` (with `--features openxr`) replaces the runtime with a
simulated headset on the desktop GPU: the eyes are drawn as in VR, the window
follows the simulated head, and a scripted controller loops through walking,
snap turns, flying, the menu, touching and sitting every 30 seconds. A
readout of the sticks and buttons sits in the window's lower left corner.

```sh
AURORA_DEMO=1 AURORA_VR_SIM=1 cargo run -p aurora-viewer --features openxr
```

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
  While the headset runs, the desktop window shows the view from its head.

## Known limits

- Each frame is drawn three times (two eyes and the window), at the window's
  size; the runtime scales the eye images to the display.
- Draw lists are culled for the window's camera, so objects outside its view
  can be missing in the headset. Mouselook (M) keeps them close.
- Only the VR menu is drawn into the headset, flat in front of the eyes;
  the rest of the interface (chat, inventory) is on the desktop window.
- There is no laser beam drawn yet for the controllers' aim.
- Temporal anti-aliasing shares its history between the eyes and the
  window; prefer FXAA or SMAA in VR.
- The eye images keep the window's size at startup: resizing the window
  afterwards crops them.
