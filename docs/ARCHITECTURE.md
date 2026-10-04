# How tg18-emu works

This page explains what is inside a Tamagotchi On / Meets and how tg18-emu
imitates it closely enough that the original game runs unchanged. The first
half is an overview for the curious; the second half ("Hardware reference"
onwards) holds the detailed notes on the chip and the firmware.

Numbers like `0xC0020000` are hexadecimal memory addresses. You can skip them
and still follow the text.

## Part 1: what is inside the toy

The toy is a small computer. Almost everything lives on one main chip, a
GeneralPlus system-on-chip (GPBT03 family), with a few helper chips around it:

```
            ┌──────────────── main chip (GeneralPlus) ─────────────────┐
 buttons ──►│  ARM processor (96 MHz)  ◄──►  1 MB working memory (SRAM)│
            │        │                                                 │
            │        ├── timers ──────────► buzzer (sound)             │
            │        ├── screen driver ───► 128×128 colour LCD         │
            │        ├── clock (time of day)                           │
            │        ├── battery meter                                 │
            │        └── serial ports ────► infrared LED               │
            └───┬───────────────────┬──────────────────┬───────────────┘
                │                   │                  │
         8 MB flash chip      RTC chip (keeps     Bluetooth chip
         (game + saves)       time while asleep)  (phone app)
```

### The processor

An ARM926, from the same family as the processors in mid-2000s phones and the
Nintendo DS, running at 96 MHz. It reads instructions one after another
("add these two numbers", "if button A is pressed, jump over there") and
carries them out. It understands two instruction sets: 32-bit "ARM" and the
more compact 16-bit "Thumb", which the game uses most of the time.

### The flash chip (8 MB)

This is what a firmware dump (`.bin` file) is a copy of. It holds the whole
game, including code, graphics and sound data, and also the save area. When
the toy saves, it rewrites part of this chip, which is why the pet survives a
battery change. The processor runs the game code directly from the flash
("execute in place") instead of copying it to memory first.

### The working memory (SRAM, 1 MB)

The processor's scratch paper. While the game runs, the pet's current state
lives here: hunger, happiness, weight, age, Gotchi Points, which menu is
open. In the English Fairy version, for example, the filled hunger hearts are
at `0xF800D869` and the Gotchi Points at `0xF800D884`. A few speed-critical
routines (interrupt handlers, the flash driver) are also copied here at boot.

### The peripherals

These are helper circuits on the chip. The processor talks to them by reading
and writing special memory addresses ("memory-mapped I/O"):

- **Buttons:** three input pins. Reading the right address tells the game
  whether A, B or C is held down.
- **Timers:** counters that run on their own and interrupt the processor at
  fixed intervals. One fires 1000 times a second (the system's heartbeat),
  another 125 times a second (the game's animation and music tick).
- **Buzzer:** there is no sound chip. A timer switches the buzzer on and off
  very fast, and how fast it switches sets the pitch. This is where the toy's
  "beepy" square-wave sound comes from.
- **Screen:** the game draws a 128×128 picture in memory, then a copy circuit
  (DMA) sends it to the LCD in one go.
- **Time-of-day clock:** hours, minutes and seconds for the game's clock.
- **RTC chip:** a separate, very low-power clock that keeps running while the
  toy sleeps and can wake it with an alarm.
- **Infrared and Bluetooth:** used for connecting with other toys and the
  phone app.

### Sleep: how time passes for the pet

When nobody presses a button for about 40 seconds, the game saves itself,
sets the RTC alarm for about a minute later and switches the processor off.
When the alarm goes off, the toy wakes up briefly, updates the pet (a bit
hungrier, a bit older), maybe calls for care with a beep, and goes back to
sleep. The screen stays dark during these wake-ups.

So the pet doesn't age from the clock alone. It ages through these
once-a-minute wake-ups. That is also why nothing happens to the pet while the
emulator is closed: nothing is doing the wake-ups. It is like taking the
batteries out.

## Part 2: how the emulator imitates it

The emulator is a program that pretends to be all of that hardware, so the
real game code from a dump runs on it unchanged. It is written in Rust and
split into two parts:

```
 tg18.exe
 ├── core/  (the "virtual toy", no Windows code in it)
 │    ├── cpu.rs        virtual ARM processor: decodes and runs each instruction
 │    ├── sys.rs        virtual memory and peripherals (buttons, timers,
 │    │                 screen, clocks, flash chip, ...)
 │    ├── machine.rs    runs the processor in small slices, handles sleep,
 │    │                 alarm wake-ups and the speed tricks below
 │    ├── sigs.rs       finds known routines in a dump by their code pattern
 │    ├── roms.rs       recognises the nine known firmware versions
 │    ├── sound.rs      turns buzzer switching into real audio samples
 │    └── save.rs,      save files and full-machine snapshots
 │        snapshot.rs
 └── desktop/  (the Windows app)
      ├── main.rs       window, menus, keyboard and mouse, timing
      ├── desk.rs       the desktop toy (shaped window you can drag around)
      ├── audio.rs      plays the sound through Windows
      ├── store.rs      settings, ROM list, save folders
      └── tray.rs, icon.rs, win32.rs
```

Keeping the core free of Windows code is what will make an Android version
possible later: only the "desktop" half needs replacing.

### Speed tricks

Imitating the chip one instruction at a time is straightforward. Doing it
fast enough, with clean sound and without wasting your PC's power, took most
of the work:

- **Skipping idle time.** Most of the time the game just waits in a loop
  ("has anything happened yet? no... no..."). The emulator runs one pass of
  the loop, checks whether anything changed, and if nothing did, jumps
  straight to the next timer event. This is where most of the speed comes
  from.
- **Shortcuts for slow jobs.** Saving goes through the flash chip a few bytes
  at a time, with dozens of register accesses per byte. The emulator
  recognises the game's "write to flash" routine and does the whole job in
  one step. Hatching an egg went from about 70 seconds to 2.
- **Finding routines by pattern.** The nine firmware versions keep the same
  routines at different addresses. Instead of a list of addresses per
  version, the emulator searches each dump for the routine's typical byte
  pattern. That is why all nine versions work with the same code.
- **Never sleep (optional).** The game keeps a small counter of idle seconds;
  the emulator quietly resets it so the screen stays on.

### How we know it is correct

An earlier prototype in Python used Unicorn, a widely used and well-tested
processor emulator. The Rust version has its own processor, and the two were
run side by side ("lockstep") over 29 test situations, about 58 million
instructions in all, comparing every register after every single
instruction. On top of that, whole life cycles were played through
automatically: egg to adult, marriage and the next generation, and death
from neglect and from repeated sickness.

### What is not emulated

Infrared and Bluetooth. Both need a partner (another toy or the phone app)
that speaks their protocol. When the game tries to use either, the emulator
stops, puts the game back to just before the button press that led there,
and shows a notice.

---

## Hardware reference

### Memory map

| Address | Block | Notes |
|---|---|---|
| `0x20000000` | SPI flash, memory-mapped | the image runs in place; reset at `0x20000048` |
| `0xF8000000` | internal SRAM | data, stacks, flash driver, interrupt handlers |
| `0xC0000000` | GPIO | port n at +n×0x20; buttons A/B/C = port 1 bits 5/6/7, active high |
| `0xC0020000` | 6 timers | +0 control (b15 pending, write 1 to clear; b14 IRQ enable; b13 run; clock sysclk/2 = 12 MHz, or sysclk/256 with b0), +8 reload, +0x10 count; IRQ 8. Timer 0 = 1 kHz OS tick, timer 2 = 125 Hz game tick |
| `0xC0020080` | timer 4 = buzzer | PWM: +8 reload = −(period), +0xC = −(high time), b13 starts/stops the tone; see Sound |
| `0xC0040000` | time-of-day clock | +0/+4/+8 seconds/minutes/hours (counting; the game sets it by writing), +0x10/+0x14/+0x18 alarm, +0x54 status (write 1 to clear), +0x58 enable; bit 1 = 1 Hz game tick; IRQ 1 |
| `0xC0060000` | UART = infrared (IrDA) | see Infrared |
| `0xC0080000` | second SPI master = Bluetooth chip | see Bluetooth |
| `0xC0090000` | RTC chip register bus | +4 address, +8 write data, +0xC command (1 write / 2 read), +0x10 ready, +0x14 read data |
| `0xC00C0000` | ADC | +4 control (b14 start, b15 done), +8 result; battery check; IRQ 29 |
| `0xC0150000` | SPI flash controller | manual mode: +0 b6 chip select, +8 TX, +0xC RX, +4 b3 busy |
| `0xD0000000` | clock/system control | +0x3C clock source status, +0x78 bit 0 = power (cleared for deep sleep) |
| `0xD0100000` | interrupt controller | +0x28 pending IRQ number, +0x30 mask, +0x38 b0 disable |
| `0xD0500000` | LCD interface | 128×128 RGB565; +0x140 control (0x81 command, 0xA1 data, 0xD1 DMA from +0x33C), +0x18C status (0x2000 vsync, 0x40 DMA done); IRQ 27 |

### RTC chip

Registers `0x30`–`0x35` are a 48-bit counter ticking at 32768 Hz
(0 = 2007-12-31). `0x10`–`0x15` stage a new value, `0x20`–`0x25` are the wake
alarm, `0x40`/`0x50` arm it, and `0x80`–`0xF7` are battery-backed memory
(a validity marker "XYZ[" and the sleep state).

The firmware's debug `printf` is compiled out (it does nothing); the
emulator hooks it to show the firmware's log messages.

### Sound

The speaker is a square-wave buzzer driven by timer 4 in PWM mode. A note
sequencer in the firmware, running on the 125 Hz timer, reprograms it for
each note. Pitch = 12 MHz / period, where period = 0x10000 − reload; +0xC
sets the high time, normally half the period (50% duty). Measured sounds
(EN Wonder Garden):

| Sound | Notes |
|---|---|
| Boot jingle | C6 D6 E6 F6 G6, C♯6 D6 E6 F6 G6 A6, C7; 128 ms per note |
| A (move) | A♯5, 32 ms |
| B (confirm) | E5 C6 E6 G6, a quick rising chirp of about 150 ms |
| C (back) | F♯5, 32 ms |

The emulator records every change to timer 4 with its exact emulated time
and turns the changes into a band-limited square wave at 44.1 kHz. Each
sample is the average of the wave over that sample's time, which keeps high
notes free of off-key aliasing. The window streams it to Windows, rendering
each piece at its true length and keeping 50–90 ms queued. `tg18cli --wav`
writes the sound to a file instead.

### Firmware behaviour worth knowing

- **Buttons are debounced:** a press makes its sound about 44 ms later, as
  on the real toy. A button must also be released for about 50 ms between
  presses. The emulator holds even a very short tap for 50 ms (no longer),
  so fast tapping in the mini games (8–10 taps a second) counts in full.
- **After a battery change,** the CONTINUE / RESET ALL menu ignores buttons
  for about 3 s after it appears.
- **Sleep:** the backlight dims about 10 s after the last button press.
  After that, the once-a-second game routine counts idle seconds in a byte
  (`0xF800D763` in the EN versions). Past 30 it requests sleep; the main loop
  then saves to flash, arms the RTC alarm and cuts the power, so the toy
  sleeps 40 s after the last press. The 2018 JP versions (v030, v031) count
  in the RTC interrupt instead, with a 180 s limit. The emulator finds the
  counter by its code pattern (`ldrb r3,[rN,#k]; cmp r3,#limit; bhi`, then
  "sleep flag = 2"), which occurs exactly once in each of the nine images.
- **The firmware won't go to sleep while the pet is calling** for attention
  (it starts to, then cancels). Closing the emulator at that moment keeps an
  autosave snapshot instead of the normal sleep-based save.
- **Saving:** the game writes its save area to flash two bytes at a time
  (130 KB when an egg hatches; about 11,800 flash operations when a new game
  formats its save area), and the flash driver needs about 65 register
  accesses per byte. The emulator runs the driver's
  `flash_write(addr, buf, len)` (found by its code in all nine images) and
  the game's halfword save loop (in the seven 2019+ images; its address
  limits are read from the firmware) directly, with NOR flash semantics
  (bits only go from 1 to 0). The game's own read-back check after each save
  still runs. Hatching went from about 70 s to 2 s, and the first PLEASE
  WAIT from 10.5 s to 4 s.
- **Clock:** the clock screen (B in the room) and the game's time of day
  read the time-of-day clock at `0xC0040000` (read by `0x20007D1C` in EN
  Magic, which also counts a day when the hour goes from 23 to 0). The game
  sets it from the RTC chip at boot and when you set the clock. The stored
  date and time the game compares against (`0xF800BE38`: month, day, hour,
  minute) is only refreshed at boot, which on a real toy means every
  wake-up, i.e. once a minute while asleep. With "never sleep" on, the
  emulator calls that refresh routine itself once a minute, at a quiet
  moment in the main loop, with all registers restored.
- **Time away:** while asleep the toy wakes on its RTC alarm about once a
  minute, updates the pet (hunger, happiness, age) and sleeps again; this is
  how time passes for the pet. Moving the clock forward alone doesn't age or
  feed it.
- **Randomness:** the main loop calls the C library's `rand()` once per pass
  and throws the result away, only to stir the sequence, so random events
  depend on exactly when buttons are pressed. `rand()`'s 64-bit state lives
  at `0xF8009AC0`.

### Bluetooth

The Bluetooth (BLE) chip sits on a second SPI master at `0xC0080000`
(+8 TX byte, +0xC bits 0–2 = byte done, +0x10 RX byte; the driver
`spi_transfer` was found in all nine images). Nothing in normal play touches
it; items and menus that talk to the phone app do (Item Box > Special >
Camera, Connection > App > Visit/Download).

It is not emulated. The window stops the game on its first transfer, puts it
back to just before the button press that led there (a snapshot is kept from
before each press, at most one per second) and shows a notice. Without that,
the chip reads as absent (0x00), start-up times out, and the firmware prints
"BLE Initial Fail" and loops forever ignoring every button. The emulator
also watches for that loop and then restarts the toy like a battery change.

For whoever tries to emulate it one day: its 16-bit status register 0x0A is
read by sending 0x0B and two dummy bytes; it reads 0x8040 when the chip is
up and 0x8000 after the start-up tables (about 345 KB, command AC 55) are
loaded; bit 14 = busy.

### Infrared

A serial port at `0xC0060000` (IrDA; +0 data, +8 control, +0xC baud rate,
+0x10 status with bit 3 = busy). Boot only configures it (+8, +0xC). The IR
code (Connection > Tamagotchi > Playdate/Gift/Marry, Connection > Download)
uses +0, +0x10, +0x14 and +0x20, then waits for a partner indefinitely,
ignoring C. The window blocks it the same way as Bluetooth.

Without the window's block, the port turns into plain memory after that
first access ("idle, nothing received"), and the IR pulse timing loops
(`mov r8,r8; subs r3,#1; cmp r3,#0; bne`, 5–6 per image) are finished in one
step whenever the emulator finds the processor inside one, so waiting runs
at about half speed instead of 1/50.

---

*This project contains no Bandai firmware, graphics or other copyrighted
material. Tamagotchi is a trademark of Bandai. This project is not
affiliated with or endorsed by Bandai.*
