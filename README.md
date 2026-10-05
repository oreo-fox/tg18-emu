<p align="center">
  <img src="docs/logo.png" alt="tg18-emu logo" width="480">
</p>

# tg18-emu

tg18-emu is a Tamagotchi ON/Meets emulator for Windows written in Rust.

## Features

- Run the emulator in a window or windowless on your Windows desktop to drag around freely
- Save any time
- Auto-saves and save slots
- Keep screen on or allow sleep to save resources (it will still beep for care calls like a real Tamagotchi).
- Different pastel colored shells/backgrounds to choose from
- Sync to system time via the menu
- Mute/Unmute any time via menu
- Different screen sizes

## Limitations

- Only available for Windows at the moment, port to Android is planned in the near future.
- No background progression while the app is closed, time can be synced/progressed but there will be no change to the Tama's hunger, happiness or aging-up (basically the same as when you take out the batteries from the real device and put them in again later on)
- Currently only "offline" play is possible. None of the Infrared/Bluetooth functions are usable. Therefore only NPCs can be married.

## Supported Firmware (ROMs)

- Tamagotchi ON Fairy (EN)
- Tamagotchi ON Magic (EN)
- Tamagotchi ON Wonder Garden (EN)
- Tamagotchi MEETS Fairy (JP)
- Tamagotchi MEETS Magical (JP)
- Tamagotchi MEETS Pastel (JP)
- Tamagotchi MEETS Sanrio (JP)
- Tamagotchi MEETS Sweets (JP)
- Tamagotchi MEETS Fantasy (JP)

Tamagotchi SOME (KR) are not supported at the moment.


## Legal

This project contains NO Bandai firmware, graphics or other copyrighted
material, only independently written emulator code and hardware notes. Do NOT
open issues or pull requests containing firmware dumps, save files, snapshots
or screenshots. Tamagotchi is a trademark of Bandai. This project is not
affiliated with or endorsed by Bandai.

The emulator code is released under the MIT License (see `LICENSE`).

## AI Disclaimer

AI was used to assist with reverse-engineering and programming. 

## Usage

### Installation

1. Download the latest release
2. Unzip the folder in a place where you want to keep it
3. Put your Firmware (ROMs) into the roms folder
4. Run tg18.exe and choose the firmware you want to play

Take note that running the executable will likely trigger a SmartScreen alert (Windows protected your PC) since this is not a known software, select "More info" and "run anyway". This should only happen the first time you run it.

####  Defender issue

tg18.exe might be falsely detected by Microsoft Defender as "Trojan:Win32/Wacatac". This is mostly my fault because I submitted an earlier build to VirusTotal without a signature, meta data or manifest, which looks suspicious to some vendors (e.g. Microsoft). If this happens you will have to restore the file from quarantine and exclude it from scanning: 

1. Windows Security (search for it in the Start menu) > Virus & threat protection > Protection history > Click the Trojan:Win32/Wacatac entry. Windows asks for admin permission; say yes > Restore

2. Go to Virus & threat protection > Manage settings > Exclusions > Add or remove exclusions > Add an exclusion > Folder, and pick your "tg18-emu-win" folder.

Sorry for the circumstances. I already submitted it to Microsoft for allowlisting but that can take time.

### How to play

The emulator will start in windowed mode by default. You can switch to "On Desktop" mode by selecting "Settings -> Put the toy on the desktop" in the top menu. From there you can drag it around your desktop freely like a virtual toy. To open the menu in desktop mode right-click anywhere on the virtual toy. 

#### Settings

File:
* Open ROM: Select a rom from your current ROM folder to run
* Open ROM folder: Open the current ROM folder
* Choose ROM folder: Choose a different ROM folder
* Save now: save current state (flash save)
* Save slot/Load slot: Save and load up to three saves to dedicated slots
* Load save file: Load a save from your folder
* Open save folder: open the folder containing all flash saves and autosaves (snapshots)
* Quit: Quit tg18-emu. Progress will be saved on quitting but your Tama's status (hunger, aging, etc.) will not advance in the background. Your last save will be loaded if you re-start the application.

Settings:
* Volume: Adjust volume directly without having to go to the in-game menu
* Mute: Mute/unmute sound instantly (you can also press M while windowed)
* Screen size: Adjust screen size
* Colour: Adjust shell/window color
* Show A, B, C buttons: Show/hide letters on the buttons
* Autosave: Enable/disable auto-saving and adjust frequency
* Never sleep: Keeps the screen on all the time if enabled. Otherwise the toy goes to sleep about 40 seconds after the last button press (3 minutes on some 2018 JP versions), like the actual device does. It will still beep on care calls if the screen is off. 
* Stop clock while closed: Clock time will not be advanced when you open the emulator next time if enabled. Take note that your Tama's state will NOT change regardless of that setting (same as if you took out the batteries on the actual device)
* Set clock to Windows time: You can use this to automatically adjust the clock to you system time. Again this will NOT change the state of your Tama (hunger, happiness, aging)
* Put the toy to the desktop: Enable desktop mode

Additional while in Desktop Mode (right-click):
* Always on top: If enabled your virtual toy will show on top of other desktop applications
* Back to the window: switch back to windowed mode

### Building (Rust)

Install Rust from https://rustup.rs (the GNU toolchain works without Visual
Studio: `rustup-init -y --default-host x86_64-pc-windows-gnu`), then in the
project folder:

    cargo build --release

This makes `target/release/tg18.exe` (the window), `tg18cli.exe` (scripted
runs) and `lockstep.exe` (the CPU check). The window app is Windows-only for
now (it uses the Windows API directly, no extra packages); the core has no
platform code apart from reading the local time.

### Making a release

    python tools/make_release.py [WHERE]

builds `tg18-emu-win` (default in `dist/`, which git ignores): a lean
`tg18.exe` without debug data, a short guide for players
(`tools/release/README.txt`), the license and empty `roms` and `saves`
folders, ready to zip. It needs no installation and only Windows' own
libraries. Settings start at the built-in defaults (window, 3x, pink,
volume 20%, never sleep off) until the player changes them.


### Saving

Saves are kept per ROM in `saves/<rom name>/`:

* `game.flash` (+ `.json`): the toy's own flash save and clock. When you
  close the window or switch ROMs, the device is put to sleep first, so the
  game saves itself exactly as the real toy does, and next time it wakes
  straight into the game.
* `autosave.t18s`: a snapshot of the
  whole machine, taken at the autosave interval (written on a background
  thread, so play doesn't stutter). If the window was not closed normally,
  the game resumes from here.
* `slot1-3.t18s` / `slot1-3.snap`: manual save slots. Loading a slot
  autosaves the current game first.

## Prototype

A prototype was written in Python first (using Unicorn) to test feasibility. While it is functional, performance is bad at times. You can just ignore it.

## How it works

Want to know what's inside the toy and how the emulator imitates it? See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for an overview with diagrams
and the detailed hardware and firmware notes.
