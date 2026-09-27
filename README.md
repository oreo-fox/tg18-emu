# tg18-emu

Experimental emulator for Tamagotchi Meets / Tamagotchi On ("tg18") firmware,
built as a feasibility study. No firmware is included; you need your own
8 MiB SPI flash dump.

Status: a playable prototype. All nine known dumps boot to the clock-setting
screen. Wonder Garden (EN v063) and Magic (EN v058) have been played from setup
(clock, birthday, name) to a hatched baby, with feeding, the menus, saves and
deep sleep/wake working. A live window (`tg18win.py`) shows the screen, takes
the A/B/C buttons, plays the buzzer sound and runs at real-time speed on
normal screens, with a ROM menu, settings, autosave and save slots.

Not emulated yet:

* Bluetooth (the phone app, the camera item) and infrared connections with
  another Tamagotchi. The window blocks them with a notice instead of letting
  the game hang.
* Time passing for the pet while the window is closed (the clock moves on, the
  pet is paused, like a toy with the batteries out).
* The backlight (dimming) and the two unidentified input pins.

## Legal

This project contains no Bandai firmware, graphics or other copyrighted
material, only independently written emulator code and hardware notes. Do not
open issues or pull requests containing firmware dumps, save files, snapshots
or screenshots. Tamagotchi is a trademark of Bandai; this project is not
affiliated with or endorsed by Bandai.

The emulator code is released under the MIT License (see `LICENSE`).

## Usage

    pip install -r requirements.txt

### Playing in a window

    python tg18win.py

The first time, it asks for the folder with your ROM dumps; after that it
reopens the last ROM and continues where you left off. `python tg18win.py
<flash.bin>` opens a specific dump. Uses only tkinter and ctypes, which come
with Python. Sound plays through Windows' built-in winmm; on other systems the
window runs silent.

Press A, B and C with the `A` `B` `C` keys, the arrow keys (Left = A,
Down = B, Right = C) or by clicking the buttons. Hold A and C together to
press both. `M` mutes, `Ctrl+S` saves.

* **File**: Open ROM (every dump in the ROM folder), Choose ROM folder,
  Save now, Save to slot / Load slot (3 slots per ROM), Open saves folder.
* **Settings**: volume, mute, screen size (2x-6x), autosave interval (off,
  1, 2, 5 or 10 minutes), Never sleep (default on) and Pause time while
  closed (default off). Settings are kept in `settings.json`.

Saves are kept per ROM in `saves/<rom name>/`:

* `game.flash` (+ `.json`): the toy's own flash save and clock. When you
  close the window or switch ROMs, the device is put to sleep first, so the
  game saves itself exactly as the real toy does, and next time it wakes
  straight into the game.
* `autosave.snap`: a snapshot of the whole machine, taken at the autosave
  interval (compressed on a background thread, so play doesn't stutter). If
  the window was not closed normally, the game resumes from here.
* `slot1-3.snap`: manual save slots. Loading a slot autosaves the current game
  first.

Bluetooth (the phone app, the camera item) and infrared connections with
another Tamagotchi are not supported: choosing one shows a notice and puts
the game back to just before you chose it.

Saves from before per-ROM folders (`saves/*.flash`) are imported automatically
the first time their ROM is opened.

Time while closed: the pet makes no progress while the window is closed, like
a Tamagotchi with the batteries out. The device clock still moves forward by
the time away, so the time of day stays right (unless Pause time while closed
is on). The firmware only counts time for the pet through the alarm wake-ups
it makes about once a minute while asleep; replaying those would take ~45 s
per 30 minutes away, so they are deliberately skipped.

The window keeps up with real time on normal screens. In the rare busy
moments when it can't (such as loading the room after CONTINUE, about 2 s at
half speed), play goes into slow motion but the device clock is sped up to
match, so the game's time of day stays in step with the real clock.

### Scripted runs

    python tg18emu.py <flash.bin> --insns 800000000 --frames frames --press 6.0:A --press 7.2:B

* `--seconds N`    emulated seconds to run (or `--insns N`)
* `--frames DIR`   save every distinct LCD frame as PNG (3x scale)
* `--press T:KEY`  press A, B or C at T seconds into this run; `A+C` presses both
* `--date "YYYY-MM-DD HH:MM"`  RTC start time (default: now, or the saved clock)
* `--save FILE`    persistent flash (the game's save data) plus `FILE.json` for
  the clock; boots from it if it exists and writes it back at the end. The clock
  keeps running while the emulator is closed, like a real device.
* `--snapshot-out FILE` / `--snapshot-in FILE`  freeze and resume the whole
  machine mid-game. Snapshots are Python pickles: only load ones you made.
* `--turbo N`      run the device clock N times faster (fast-forward game time)
* `--wav FILE`     write the run's buzzer sound to a WAV file (silences longer
  than 2 s, such as deep sleep, are shortened)
* `--never-sleep`  keep the device awake (resets the firmware's idle counter)
* `--trace-mmio`   print the first access to every hardware register

Deep sleep is emulated: after about a minute without input the firmware arms
the RTC alarm and cuts power. The emulator then skips straight to the alarm
(or the next scripted button press) and cold-boots the CPU, keeping flash, the
RTC and its battery-backed registers, like the real chip. Sleep costs no
emulation time.

* `--no-idle-skip` always execute idle loops (for comparing behaviour)

Speed: idle loops are detected by running one more pass of the current loop;
if registers, internal RAM and hardware writes are unchanged, emulated time
jumps to the next interrupt or button event. Typical screens then run faster
than real time (about 2x in the menus, 1.7x in the room with the pet). The
main loop calls rand() on every pass only to stir the sequence and discards the
result, so rand()'s state (found by its code signature, present in all nine
images) is left out of the comparison. Registers are read with one direct call
into Unicorn's C API, as its Python wrapper costs ~35 us per read. Genuinely
CPU-heavy stretches (loading a screen, formatting flash) run at about 0.4-0.5x,
limited by Unicorn's instruction-count slicing in Python. Deep sleep costs
nothing. The emulator never writes to the dump file. Saves and
snapshots refuse to load with a different ROM version.

## Controls learned so far (EN Wonder Garden)

* Clock / birthday setup: A changes the blinking field, B confirms it.
* Name entry: A/C move the cursor right/left, B edits a slot. In the picker,
  A cycles pages (letters, digits, symbols), B picks. Moving past the last slot
  opens CONFIRM? YES/NO.
* Main screen: A opens the 10-icon menu and moves between icons (it remembers
  the last one), B selects, C goes back. Icons: Profile Setting, Meal&Snack,
  Bathroom, Connection, (house), Explore My Town, Travel to Other Town,
  Item Box, Notebook, First Aid Kit.
* Feeding: Meal&Snack > Fridge > food > B. Status: Profile Setting > Profile,
  A flips pages.

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
| 0xC0040000 | RTC interrupts | +0x54 status (W1C), +0x58 enable; bit 1 = game-time tick (1 Hz assumed); IRQ 1 |
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
  real toy.
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
* While asleep the toy wakes on its RTC alarm about once a minute, updates the
  pet (hunger, happiness, age) and sleeps again; this is how time passes for
  the pet. Moving the clock forward alone doesn't age or feed it.
* The main loop calls newlib's rand() once per pass and discards the result,
  only to stir the sequence, so random events depend on when buttons are
  pressed. rand()'s 64-bit state lives in the reent struct (+0xA8), at
  0xF8009AC0 in Wonder Garden.
