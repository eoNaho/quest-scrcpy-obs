# Quest scrcpy

A native, from-scratch [scrcpy](https://github.com/Genymobile/scrcpy) client written in Rust and tuned for the **Meta Quest 3** — it renders the headset view in its own window, with a live one-eye crop, lens (fisheye) flattening, audio, screenshots and MP4 clip recording.

It is **not** a GUI wrapper around `scrcpy.exe`. It talks the scrcpy client/server protocol directly, decodes H.264 with the built-in **Windows Media Foundation** decoder, and renders with **egui/wgpu**. No ffmpeg, no openh264 — the scrcpy server `.jar` is embedded straight into the executable, so it's a single self-contained `.exe` (you only need `adb` on your `PATH`).

## Why this exists

On Horizon OS 74+, plain scrcpy shows a **black screen / flicker** on Quest. The cause ([scrcpy#5913](https://github.com/Genymobile/scrcpy/issues/5913)) is that the Quest's `createVirtualDisplay()` throws an exception but starts mirroring anyway, so scrcpy's `SurfaceControl` fallback opens a *second* capture on the same surface. This client ships the **official scrcpy v4.0 server**, which detects Quest devices and skips that broken fallback — so it actually renders.

On top of that, the Quest mirrors the stereoscopic, lens-distorted view. This client lets you crop to one eye and **flatten the fisheye** into a clean, upright 2D image.

## Features

- 🛰️ **Flat view (unrooted)** — one click streams the *whole* undistorted Quest view (home environment + floating panels, exactly like Meta's casting), live. A tiny on-device agent (pushed and run via `adb`, just like the scrcpy server — **no root**) composites the flat view and encodes H.264, so there's no lens de-warp to fiddle with.
- 🎯 Device & display pickers over adb (shows every display the Quest exposes)
- 📶 **Wireless adb** — connect a Quest by `ip[:port]`; remembered addresses get one-click reconnect chips + "Connect all"
- 🥽 **One-click Quest 3 preset** — left eye, lens flattened, leveled
- 🔍 Live crop: drag to pan, scroll to zoom, eye/full presets
- 🪞 **Flatten lens**: live radial-distortion + tilt correction (de-fisheye the VR view)
- 🔊 Audio toggle (AAC via Media Foundation → cpal)
- 📷 Screenshot the current crop to PNG
- ⏺ **Record** to `.mp4` — captures exactly the processed view you see (cropped, lens-flattened, tilted); the CLI can also do a lossless full-panel passthrough
- 📡 **Send to OBS (Spout2)** — one click exposes the current view as a live Spout2 sender named "Quest scrcpy", so OBS can pick it up as a source with no Window Capture and no re-encoding
- 🎥 **Virtual camera** — one click exposes the current view as a real system webcam (via the OBS Virtual Camera device), usable in Zoom, Discord, browsers, or OBS itself — no Spout plugin needed
- 🥷 **Streamer mode** — auto-hides device serials and saved Wi-Fi addresses from the UI whenever OBS is running (so they don't leak into a Window/Display Capture), with a manual override
- ⚡ Low-latency pipeline: `MF_LOW_LATENCY` decode, no-vsync present, multi-threaded NV12→RGBA
- 💾 Settings (lens/tilt/crop/quality + remembered devices) auto-saved and restored between launches
- 🖥️ Both a GUI and a CLI

## Requirements

- Windows 10/11 (uses Media Foundation)
- `adb` on your `PATH` (Android platform-tools)
- A Quest in developer mode, connected over USB or wireless adb

## Usage

GUI (default):

```sh
quest-scrcpy            # launch the GUI
quest-scrcpy gui        # same
quest-scrcpy mirror --serial <SERIAL> --audio   # launch + auto-connect
```

CLI helpers:

```sh
quest-scrcpy list                       # list adb devices
quest-scrcpy connect 192.168.1.40       # adb connect over Wi-Fi (defaults to :5555)
quest-scrcpy displays --serial <SERIAL> # list device displays
quest-scrcpy record --serial <SERIAL> -n 15 -o clip.mp4               # 15s full-panel clip (lossless)
quest-scrcpy record --serial <SERIAL> --crop left --flatten -o eye.mp4 # one-eye, lens-flattened clip
quest-scrcpy shot --serial <SERIAL> -o frame.png                      # single-frame grab
```

In the GUI: pick your Quest, hit **Connect**, click **🥽 Quest 3** for the tuned view, then fine-tune **Flatten lens** (`curve`/`edge`) and `tilt°` if needed. Screenshots and clips land in a `captures/` folder next to the executable.

## Streaming to OBS

1. Install the [Spout2 OBS plugin](https://github.com/Off-World-Live/obs-spout2-plugin/releases) (unzip the `win-spout` folder into `C:\ProgramData\obs-studio\plugins`) and restart OBS.
2. In quest-scrcpy, connect to your Quest and click **📡 OBS** in the toolbar. It turns into **📡 OBS: on** and starts sending the exact view you see (crop, lens-flatten, tilt included — or the full flat view when using **🥽 Flat view**).
3. In OBS, add a source → **Spout2 Capture** → pick **Quest scrcpy** from the sender list.

No Window Capture, no re-encode: OBS reads the frame straight off the GPU/CPU share, so it updates live as you drag the crop or tweak the lens.

## Virtual camera

Click **🎥 Virtual cam** in the toolbar to expose the current view as a real webcam, usable in any app — Zoom, Discord, a browser tab, or OBS via a **Video Capture Device** source.

- Requires **OBS to be installed** (even if not running) — its installer is what registers the "OBS Virtual Camera" device other apps see; this feature just writes into that same device directly.
- It's the *same* device OBS's own **Start Virtual Camera** button feeds, so don't run both at once — whichever starts first grabs it, the other fails to open.
- No extra plugin to install (unlike Spout2), since it reuses OBS's own driver.

## Streamer mode

Device serials and remembered Wi-Fi addresses (`ip:port`) are personally-identifying hardware info with no reason to show up on stream. **🥷 Streamer mode** hides them automatically whenever it detects `obs64.exe` running (checked every couple seconds), replacing them with `•••` in the device picker and the Wi-Fi reconnect chips. Click the toolbar button to override it: cycles **Auto → always on → always off → Auto**.

## Build

```sh
cargo build --release
# -> target/release/quest-scrcpy.exe
```

## How it works

- `adb.rs` — push the embedded server jar, set up the forward, launch `app_process`, list displays
- `server.rs` — scrcpy v4.0 wire protocol (handshake, session-meta resolution, packet framing)
- `decoder.rs` — H.264 → NV12 → RGBA via the Media Foundation H.264 MFT
- `recorder.rs` — H.264 → MP4 mux via the Media Foundation sink writer
- `audio.rs` / `audioplay.rs` — AAC → PCM via MF, played through cpal
- `stream.rs` — the streaming thread tying it together
- `app.rs` — the egui front-end (crop, lens flatten, capture controls, **🥽 Flat view**)
- `flat.rs` — the unrooted flat-view source: pushes/runs the on-device agent, pipes its H.264 over `adb exec-out`, decodes it
- `spout.rs` — feeds the same processed BGRA frame the screen/recorder use into a Spout2 sender, so OBS can capture it as a source
- `vcam.rs` — feeds the same processed RGBA frame into the OBS Virtual Camera device, so it shows up as a system webcam
- `obsdetect.rs` — a Toolhelp32 process-list check for `obs64.exe`, driving streamer mode's auto-hide
- `agent/` — the on-device agent (`FlatStream`): an `app_process` (shell uid) that captures the whole flat view via `MediaProjectionManagerExt` + `MediaCodec`. Built to `assets/quest-flat-agent.jar`

## Third-party

This project embeds and launches the scrcpy server from [Genymobile/scrcpy](https://github.com/Genymobile/scrcpy) (Apache-2.0). See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

## License

MIT — see [LICENSE](LICENSE).

**Exception**: the **🎥 Virtual cam** feature depends on the [`virtualcam`](https://crates.io/crates/virtualcam) crate, which is **AGPL-3.0** licensed. Building this project with that feature included means the *resulting binary* carries AGPL-3.0 obligations (e.g. source availability) on top of the MIT license — this is a deliberate tradeoff made to get a real system-wide virtual camera without writing/registering a driver from scratch. See [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md) for details.
