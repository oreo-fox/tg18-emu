# tg18-emu

Experimental emulator for Tamagotchi Meets / Tamagotchi On ("tg18") firmware,
built as a feasibility study. No firmware is included; you need your own
8 MiB SPI flash dump.

Status: a playable prototype. All nine known dumps boot to the clock-setting
screen. The Wonder Garden (EN v063) image has been played through setup (clock,
birthday, name) to a hatched baby, feeding and the menus, with saves and deep
sleep/wake working. A live window (`tg18win.py`) shows the screen, takes the
A/B/C buttons and plays the buzzer sound at real-time speed on normal screens.

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

    python tg18win.py <flash.bin> --save saves/mygame.flash

Shows the screen at 4x and plays live. Press A, B and C with the `A` `B` `C`
keys, the arrow keys (Left = A, Down = B, Right = C) or by clicking the
buttons. Hold A and C together to press both, and press `M` to mute. The save
is written when the device goes to sleep and when you close the window. Options: `--save FILE`,
`--snapshot-in FILE`, `--snapshot-out FILE`, `--date`, `--scale N`,
`--volume 0-100`. Uses only tkinter and ctypes, which come with Python. Sound
plays through Windows' built-in winmm; on other systems the window runs silent.

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
* The main loop calls newlib's rand() once per pass and discards the result,
  only to stir the sequence, so random events depend on when buttons are
  pressed. rand()'s 64-bit state lives in the reent struct (+0xA8), at
  0xF8009AC0 in Wonder Garden.
