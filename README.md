# Bluekey



(I will finish this probably like, tomorrow(2026-07-20 0141 CST now) or something)
Presently, a simple command line tool for forwarding either a keyboard or a mouse over Bluetooth to another device from a Linux computer.

Only Bluekeyd is implemented now, as a simple CLI. Bluekeyd will likely either need to run as root, in the input group, or otherwise have access to the /dev/input/* devices chosen directly.
`sudo --preserve-env setpriv --regid $(id -g $USER) --reuid $(id -u $USER) --groups input,$(id -G $USER | sed "s/ /,/g") bash` is a fairly easy way to start a shell with the input group.

Eventually, Bluekeyd will be turned into a system dameon to both allow unprivliged users to initiate passthrough and avoid starting and restarting the Bluetooth keyboard and mouse server everytime.
Currently tested running on Arch Linux, connecting to with an iPad. 

Pass a keyboard or mouse through an emulated Bluetooth device
```
Usage: bluekeyd [OPTIONS] <--keyboard <KEYBOARD>|--mouse <MOUSE>>

Options:
  -k, --keyboard <KEYBOARD>  Path to keyboard device to forward
  -m, --mouse <MOUSE>        Path to mouse device to forward
      --skip-wait            Skip the short wait before grabing the keyboard, to avoid a stuck enter key
  -h, --help                 Print help (see more with '--help')
```

If you couldn't tell, this is a *very* quick and dirty readme. 
