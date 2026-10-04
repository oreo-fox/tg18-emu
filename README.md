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
- No background progression while the app is closed, time can be synced/progressed but there will be no change to the Tama's hunger, happiness or aging-up (basically the same as when you take out the batteries form the real device and put them in again later on)
- Currently only "offline" play is possible. None of the Infrared/Bluetooth functions are usable. Therefore only NPCs can be married.

## Supported Firmware (ROMs)

- Tamagotchi ON Fairy (EN)
- Tamagotchi ON Magic (EN)
- Tamagotchi ON Wonder Garden (EN)
- Tamagotchi MEETS Fairy (JP)
- Tamagotchi MEETS Magic (JP)
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

AI was used to assist with reverse-enginering and programming. 

## Usage

### Installation

1. Download the latest release
2. Unzip the folder in a place where you want to keep it
3. Put your Firmware (ROMs) into the roms folder
4. Run tg18.exe and choose the firmware you want to play

Take note that running the executable will likely trigger a SmartScreen alert (Windows protected your PC) since this is not a known software, select "More info" and "run anyway". This should only happen the first time you run it.

### How to play

The emulator will start in windowed mode by default. You can switch to "On Desktop" mode by selecting "Settings -> Put the toy on the desktop" in the top menu. From there you can drag it around your desktop freely like a virtual toy. To open the menu in desktop mode right-click anywhere on the virtual toy. 

#### Settings

File:
* Open ROM: Select a rom from your current ROM folder to run
* Open ROM folder: Open the current ROM folder
* Choose ROM folder: Choose a different ROM folder
* Save now: save current state (flash save)
* Save slot/Load slot: Save and load up to three saves to dedicated slots
* Open save folder: open the folder containing all flash saves and autosaves (snapshots)
* Quit: Quit tg18-emu. Progress will be saved on quitting but your Tama's status (hunger, aging, etc.) will not advance in the background. Your last save will be loaded if you re-start the application.

Settings:
* Volume: Adjust volume directly without having to go to the in-game menu
* Mute: Mute/unmute sound instantly (you cam also press M while windowed)
* Screen size: Adjust screen size
* Colour: Adjust shell/window color
* Show A, B, C buttons: Show/hide letters on the buttons
* Autosave: Enable/disable auto-saving and adjust frequency
* Never sleep: Keeps the screen on all the time if enabled. Otherwise the screen goes to sleep after 1 minute like the actual device does. It will still beep on care calls if the screen is off. 
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

A prototype was written in Python first to test feasability. While it is functional, performance is quite bad. You can just ignore it.

## Hardware notes (GeneralPlus GPBT03-family SoC, ARM926-class core)

| Address | Block | Notes |
|---|---|---|
| 0x20000000 | SPI flash, memory-mapped | image runs in place; reset at 0x20000048 |
| 0xF8000000 | internal SRAM | .data/.bss/stacks, flash driver, ISRs |
| 0xC0000000 | GPIO | port n at +n*0x20; buttons A/B/C = port 1 bits 5/6/7, active high |
| 0xC0020000 | 6 timers | +0 ctrl (b15 pending W1C, b14 IRQ en, b13 run, clock sysclk/2 = 12 MHz or sysclk/256 with b0), +8 reload, +0x10 count; IRQ 8. Timer 0 = 1 kHz OS tick |
| 0xC0020080 | timer 4 = buzzer | PWM: +8 reload = -(period), +0xC = -(high time), b13 starts/stops the tone; see Sound |
| 0xC0090000 | RTC register bus | +4 addr, +8 wdata, +0xC cmd (1 wr/2 rd), +0x10 ready, +0x14 rdata |
| 0xC00C0000 | ADC | +4 ctrl (b14 start, b15 done), +8 result; battery check; IRQ 29 |
| 0xC0150000 | SPI flash controller | manual mode: +0 b6 CS, +8 TX, +0xC RX, +4 b3 busy |
| 0xC0040000 | SoC real-time clock | +0/+4/+8 seconds/minutes/hours (counts; the game sets it by writing), +0x10/+0x14/+0x18 alarm, +0x54 status (W1C), +0x58 enable; bit 1 = game-time tick (1 Hz assumed); IRQ 1 |
| 0xD0000000 | clock/system | +0x3C clock source status, +0x78 bit 0 = power (cleared for deep sleep) |
| 0xD0100000 | interrupt controller | +0x28 pending IRQ number, +0x30 mask, +0x38 b0 disable |
| 0xD0500000 | LCD interface | 128x128 RGB565; +0x140 ctrl (0x81 cmd, 0xA1 data, 0xD1 DMA from +0x33C), +0x18C status (0x2000 vsync, 0x40 DMA done); IRQ 27 |

RTC: registers 0x30-0x35 are a 48-bit 32768 Hz counter (0 = 2007-12-31),
0x10-0x15 stage a new value, 0x20-0x25 are the wake alarm, 0x40/0x50 arm it,
and 0x80-0xF7 are battery-backed memory (validity marker "XYZ[", sleep state).
The firmware's printf is compiled out; the emulator hooks it to show the log.

### Sound

The speaker is a square-wave buzzer driven by timer 4 in PWM mode, and a note
sequencer in the firmware (on the 125 Hz timer) reprograms it for each note.
Pitch = 12 MHz / period, where period = 0x10000 - reload; +0xC sets the high
time, normally half the period (50% duty). Measured sounds (EN Wonder Garden):

| Sound | Notes |
|---|---|
| Boot jingle | C6 D6 E6 F6 G6, C#6 D6 E6 F6 G6 A6, C7; 128 ms per note |
| A (move) | A#5, 32 ms |
| B (confirm) | E5 C6 E6 G6, a quick rising chirp of about 150 ms |
| C (back) | F#5, 32 ms |

The emulator records every change to timer 4 with its emulated time and turns
them into a band-limited square wave at 44.1 kHz (each sample is the average of
the wave over the sample's time, which keeps high notes free of off-key
aliasing). The window streams it through winmm, rendering each span at its
true length and keeping 50-90 ms queued; `tg18emu.py --wav` writes it to a file.

### Firmware behaviour worth knowing

* Buttons are debounced: a press makes its sound about 44 ms later, as on the
  real toy. A button must also be released for about 50 ms between presses.
  The window holds even a very short tap for 50 ms (no longer), so fast
  tapping in the mini games (8-10 taps a second) is counted in full.
* The CONTINUE / RESET ALL menu after a battery change ignores buttons for about
  3 s after it appears.
* Sleep: the backlight dims about 10 s after the last button press. After
  that, the once-a-second game routine counts idle seconds in a byte
  (0xF800D763 in the EN builds) and, past 30, sets SleepFlag = 2; the main loop
  then saves to flash (ParamSave), arms the RTC alarm and cuts the power, so
  the toy sleeps 40 s after the last press. The 2018 JP builds (v030, v031)
  count in the RTC interrupt instead, with a 180 s limit. The emulator finds
  the counter by its code pattern (`ldrb r3,[rN,#k]; cmp r3,#limit; bhi`,
  then `SleepFlag = 2`), found once in each of the nine images.
* Saving: the game writes its save area to flash two bytes at a time (130 KB
  when an egg hatches, about 11,800 flash operations when a new game formats
  its save area), and the SPI driver needs ~65 register accesses per byte. The
  emulator runs the driver's `flash_write(addr, buf, len)` (found by its code,
  in all nine images) and the game's halfword save loop (in the seven 2019+
  images; its address limits are read from the firmware) directly, with NOR
  semantics (bits only go from 1 to 0). The game's own read-back check after
  each save still runs. Hatching went from ~70 s to ~2 s, and the first
  PLEASE WAIT from 10.5 s to 4 s.
* Bluetooth: the BLE chip sits on a second SPI master at 0xC0080000 (+8 TX
  byte, +0xC bits 0-2 = byte done, +0x10 RX byte; driver `spi_transfer` found
  in all nine images). Nothing in normal play touches it; items and menus that
  talk to the phone app do (Item Box > Special > Camera, Connection > App >
  Visit/Download). It is not emulated. The window stops the game on its first
  transfer, puts it back to just before the button press that led there (a
  snapshot is kept from before each press, at most one per second) and shows
  a notice. Without that, the chip reads as absent (0x00), start-up times out
  and the firmware prints "BLE Initial Fail" and loops forever ignoring every
  button; the emulator also watches for that loop (then the window restarts
  the toy like a battery change). Its 16-bit status register 0x0A is read by
  sending 0x0B and two dummy bytes: 0x8040 when the chip is up, 0x8000 after
  the start-up tables (~345 KB, command AC 55) are loaded, bit 14 = busy.
* Infrared: a serial port at 0xC0060000 (IrDA; +0 data, +8 control, +0xC baud,
  +0x10 status with bit 3 = busy). Boot only configures it (+8, +0xC); the IR
  code (Connection > Tamagotchi > Playdate/Gift/Marry, Connection > Download)
  uses +0, +0x10, +0x14, +0x20 and then waits for a partner indefinitely,
  ignoring C. The window blocks it the same way as Bluetooth. Otherwise the
  port becomes plain memory after that first access ("idle, nothing
  received") and the IR pulse timing loops (`mov r8,r8; subs r3,#1; cmp r3,#0;
  bne`, 5-6 per image) are finished in one step when the CPU is found inside
  one between slices, so waiting runs at ~0.5x instead of ~0.02x. (Code hooks
  on those loops cost ~20% speed everywhere even when unused, so they are not
  hooked.)
* The firmware won't go to sleep while the pet is calling for attention (it
  starts to, then cancels). Closing the window then keeps an autosave snapshot
  instead of the sleep-based save.
* The clock screen (B in the room) and the game's time of day read the SoC
  clock at 0xC0040000 (+0/+4/+8 = seconds/minutes/hours, read by 20007D1C in
  EN Magic, which also counts a day when the hour goes 23 -> 0). The game
  sets it from the RTC chip at boot and when you set the clock. The stored
  date/time the game compares against (0xF800BE38: month, day, hour, minute)
  is only refreshed at boot (routine found by code pattern in all nine
  images), which on a real toy is every wake-up, i.e. once a minute while
  asleep. With never-sleep the emulator calls that routine itself once a
  minute, at a quiet moment in the main loop, with all registers restored.
* While asleep the toy wakes on its RTC alarm about once a minute, updates the
  pet (hunger, happiness, age) and sleeps again; this is how time passes for
  the pet. Moving the clock forward alone doesn't age or feed it.
* The main loop calls newlib's rand() once per pass and discards the result,
  only to stir the sequence, so random events depend on when buttons are
  pressed. rand()'s 64-bit state lives in the reent struct (+0xA8), at
  0xF8009AC0.
