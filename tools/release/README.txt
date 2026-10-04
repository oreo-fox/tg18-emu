tg18-emu - Tamagotchi Meets / On emulator for Windows
=====================================================

Test version. Thank you for trying it!


WHAT YOU NEED

Your own flash dump of a Tamagotchi Meets / On (an 8 MB .bin file).
No firmware comes with this program, and dumps may not be passed on.

Supported: the EN Fairy, Magic and Wonder Garden versions and the
JP Fairy, Magical, Fantasy, Pastel, Sanrio and Sweets versions.


STARTING

1. Put your .bin dump into the "roms" folder.
2. Double-click tg18.exe.

The first time, Windows may say "Windows protected your PC" (the program
is not signed by a known publisher). Click "More info", then "Run anyway".

With one dump in "roms" the game starts by itself. With several, choose
one under File > Open ROM.


PLAYING

  Buttons:  A, B, C keys   or   Left / Down / Right arrow keys
            or click the buttons with the mouse
  A + C:    hold both keys together
  M:        sound on/off
  Ctrl+S:   save now

The game saves by itself. Close the window normally (the X) and next time
it carries on where you left off. While the program is closed, the pet is
paused (like a toy without batteries) but the clock keeps going.

Settings menu: volume, screen size, colour, letters on the buttons,
autosave, and "Set the toy's clock to Windows time now".

The screen goes to sleep after about 40 seconds without a button press,
like on the real toy. Any button wakes it. (Settings > Never sleep keeps
it on.)


DESKTOP TOY

Settings > Put the toy on the desktop turns the window into a little toy
that sits on your desktop:
  - drag it anywhere by its shell
  - right-click it for the menu (also "Back to the window")
  - the rainbow egg next to the clock (bottom right of the taskbar)
    shows or hides it
  - it wiggles when your pet calls for care


NOT SUPPORTED (YET)

Bluetooth (the phone app, the camera item) and infrared connections with
another Tamagotchi. Choosing one shows a notice and puts the game back to
just before.


FILES

  tg18.exe       the program (nothing to install)
  roms\          your dumps
  saves\         your saves, one folder per dump
  settings.json  your settings (appears after the first start)
  tg18.log       only appears if something went wrong

Please send tg18.log along if you run into a problem.


This program is not made or endorsed by Bandai. Tamagotchi is a trademark
of Bandai. The emulator itself is free software (MIT License, see
LICENSE.txt).
