<p align="center"><img src="src-tauri/icons/128x128@2x.png" width="128" alt="DeskSpan icon"></p>

# DeskSpan

Use a phone, tablet, iPad or spare computer as a second screen for your Windows, macOS or Linux computer: **mirror** or **extend** your desktop over Wi-Fi or a cable, with touch and pen control and sound. The other device just opens a link in its browser: nothing to install there.

Part of **DeskLab**, free tools by the same developer: [getdesklab.com](https://getdesklab.com/).

> **Status: before the first release.** Mirror and extend work (extend tested on macOS; Windows uses the Virtual Display Driver), over Wi-Fi or an Android USB cable, with touch, pen and mouse control back on Windows and macOS. Sound and Linux control are still to come. There is no release yet.

## How it will work

1. Open DeskSpan on the computer and press **Start sharing**.
2. On the phone or tablet, scan the QR code (or open the link and type the 6-digit code).
3. Allow the device on the computer, optionally with control (touch, pen, mouse).
4. The device now shows your screen. Allowed devices reconnect by themselves.

## Privacy and security

- Everything stays on your own network: no cloud, no account, no relay server. Video and input are encrypted by WebRTC.
- DeskSpan only listens while you share, and by default only on home/office network addresses and this computer (not VPNs or public addresses; you can turn VPNs on in Settings).
- Every new device needs the one-time QR link or code **and** your OK on the computer. Each allowed device gets its own key; the computer only stores a hash of it. Remove a device any time under Devices.
- Control of the computer is off for a device until you allow it.

## What works

| | Windows | macOS | Linux |
|---|---|---|---|
| Mirror a screen | yes | yes | yes |
| Extend (an extra monitor) | yes, with the [Virtual Display Driver](https://github.com/VirtualDrivers/Virtual-Display-Driver) (MIT, installed once with your OK) | yes | not yet |
| Touch, pen, mouse and scroll back | yes | yes (allow DeskSpan under Privacy & Security › Accessibility) | not yet |
| Android USB cable (adb) | yes | yes | yes |
| Sound | not yet | not yet | not yet |

The device can be any phone, tablet, iPad or computer with a current browser, or the **DeskSpan Screen** Android app (`mobile/`). DeskLab Link on the phone's home page also sends files to the computer's Downloads.

## Still to do

- Sound; Linux extend and control; adaptive quality and a latency read-out.
- Signed Windows installer (SmartScreen), then the first release.

## Develop

Needs Node 22+ and Rust (stable).

```sh
npm install
npm run tauri dev          # the app
npm run build              # app + receiver page (dist/, dist/receiver/)
npm test                   # unit tests
node tools/dev-host.mjs    # stand-in host for the receiver page (testing only)
```

Layout: `src/` the app's window (React), `receiver/` the page devices open (plain TypeScript), `src-tauri/` the Rust side (`server.rs` pairing + signalling, `devices.rs` allowed devices).

## Licence

MIT. Made by [Jagdeep Sandhu](https://github.com/imjagdeep).
