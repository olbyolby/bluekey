
# Bluekey
Bluekey is a command line utility enabling a computer to act as a Bluetooth keyboard and mouse, allowing a single keyboard and mouse to be used with any number of devices supporting Bluetooth.

Bluekey operates as a system daemon, accessible via DBus, enabling multiple keyboards or mice to be bridged to multiple Bluetooth clients.

## Usage 
The Bluekey client can be used as follows.
```
Usage: bluekey <COMMAND>
Commands:
  bridge           Pass a keyboard or mouse through an emulated Bluetooth device
  list             List all devices known to Bluekey as listening for keyboard or mouse input
  escape-shortcut  Set or view the keyboard escape shortcut, used for breaking the keyboard grab from the keyboard
  help             Print this message or the help of the given subcommand(s)
```
### Creating a bridge
```
Usage: bluekey bridge <--keyboard <KEYBOARD>|--mouse <MOUSE>> <--mac <MAC>|--alias <ALIAS>>
Options:
      --keyboard <KEYBOARD>  Path to keyboard device to forward(/dev/input/*)
      --mouse <MOUSE>        Path to mouse device to forward(/dev/input/*)
      --mac <MAC>            MAC address of device to bridge input to
      --alias <ALIAS>        Name/alias of device to connect to
  -h, --help                 Print help (see more with '--help')
```

### Listing devices
```
Usage: bluekey list [OPTIONS]

Options:
  -d, --detailed  
  -h, --help      Print help
```

Bluekey can also provide a list of all connected Bluetooth devices accepting either keyboard or mouse input.
Running this command without arguments will result in a comma seperated list of MAC addresses, and passing `-d` will show each device's MAC address, it's name, and if it accepts keyboard or mouse input.
Additionally, Bluetooth HID clients report their power status back to the device(active/sleeping). Sleeping devices will have their MAC address dispayed as gray in the device list.

### Connecting a device
1. To connect a Bluetooth device, you should first start the `bluekeyd` dameon, which you can run either as root or with the input group(see [staring the daemon](#starting-the-daemon))
2. You should then pair your device with your computer using either your desktop environment's GUI or something like `bluetoothctl`. Note either the device's name or it's MAC address(see [listing devices](#listing-devices))
3. Find the `/dev/input/event*` device of your chosen keyboard/mouse, `sudo evtest` can be very useful for this. If you have multiple keyboards or mice, you can chose to use a specific one.
4. Once you have your Bluetooth device's alias/mac and your keyboard and or mouse's event device, start briding them via `bluekey bridge <--keyboard <KEYBOARD>|--mouse <MOUSE>> <--mac <MAC>|--alias <ALIAS>>`
5. Your keyboard/mouse should now be connected to the Bluetooth device and allow you to interact with it.
6. Since your keyboard/mouse is being forwarded by `bluekey`, you can't interact with your main OS using it. You can use the escape-shortcut to stop keyboard forwarding(leftmeta/windows + esc by default)
7. If you have multiple input devices and clients, you can create multiple bridges to different devices simultaniously, if so inclined.
8. You can also change the keyboard escape shortcut via the `bluekey escape-shortcut [SHORTCUT]` command

*Some devices behave poorly if the keyboard and mouse services are not avalible during pairing(ie Windows), so I suggesting starting `bluekeyd` before pairing, but it is not strictly required for all devices.

### Starting the daemon
The daemon can be started by running it's command without any arguments, `bluekeyd`. To access `/dev/input devices`, it will need to either be run as root, or, more perferable, as part of the input group, using something like `sudo --preserve-env setpriv --regid $(id -g $USER) --reuid $(id -u $USER) --groups input,$(id -G $USER | sed "s/ /,/g") bash` to use the group temporarily, or a custom user. 


## Implementation
Bluekey works by hosting a standards-compliant GATT HID service, which is the standard Bluetooth service used by the majority of modern Bluetooth keyboards and mice, and then forwarding input events from the computer over HID as if it were a real input device. Consequently, a computer running Bluekey, to most connected devices, is close to indistinguishable from a real keyboard and mouse, and should work on any platform supporting those devices over Bluetooth. This allows Bluekey to be used almost universally, and without any special software or configuration on remote devices, from PS4s to iPads, or even other computers. 

| Feature             | Bluekey         | [HID Client](https://github.com/4ndrej/hidclient) | [EmuBTHID](https://github.com/Alkaid-Benetnash/EmuBTHID) | [Bluetooth Keyboard](https://github.com/SySS-Research/bluetooth-keyboard-emulator) |
| ------------------- | --------------- | ------------------------------------------------- | -------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| HID Specification   | HID over GATT   | BT/EDR Classic HID                                | BT/EDR Classic HID                                       | BT/EDR Classic HID                                                                 |
| Root Permissions    | No(Input group) | Yes                                               | Yes                                                      | Yes                                                                                |
| Manual Config       | No              | Yes                                               | Yes                                                      | Yes                                                                                |
| Shell/GUI Interface | Shell, DBus     | Shell                                             | Xorg(GUI)                                                | Shell                                                                              |
| Platforms           | Linux           | Linux                                             | Linux                                                    | Linux                                                                              |
| Multidevice         | Yes             | No                                                | No                                                       | No                                                                                 |

Compared to other software developed for this purpose, Bluekey is intended be simpler to implement, easy to use/configure, and to integrate well with both command line and graphical interfaces. Furthermore, Bluekey supports operating as a background daemon, controllable from both DBus and the CLI, allowing you to disconnect, switch devices, or do other activity without constantly starting and stopping the keyboard and mouse services. Running the Bluetooth service continuously eliminates poor behavior in some devices(like Windows computers) when the HID service starts and stops, while also allowing better ingration with other software.

Bluekey also makes use of the [Bluetooth HID over GATT standard](https://www.bluetooth.com/specifications/specs/hid-over-gatt-profile-hogp/), instead of the [Bluetooth Classic HID service](https://www.bluetooth.com/specifications/specs/human-interface-device-profile1-1-2/), which exposes a significantly simpler implementation, reducing bugs and complexity, and, owing to BT Low Energy being more widely used by modern Bluetooth peripherals, thus better supported by software vendors, should experience better support with fewer comparability or implementation errors(like EmuBTHID failing with Apple devices).

## Planned features:

1. I plan to write a desktop applet for interacting with Bluekey
2. I'm not sure about a good way to document the DBus interface and sync with zbus
3. The dameon could use better error handling

## Motivation 
The primary motivation behind this project is to address situations where a user may be operating multiple devices that they want to use a keyboard or mouse with simultaneously, but without the difficulty of having 2 sets of input devices on their desk, or worse, constantly unpairing and repairing devices, or shuffling around USB plugs. In particular, I(@olbyolby) am using this with my iPad, which I use for art, and I prefer to use a keyboard for input, but don't want to have 2 keyboards on my desk. However, this software should be useful for any other circumstance you need a wireless keyboard/mouse, perhaps if you have a device requiring Bluetooth, but only have wired inputs, or if you wanted use a keyboard/mouse with a console but without having to buy a new keyboard.

I found pre-existing solutions to often be outdated, or to require difficult configuration and expose potential security risks or poor practices(for instance, many require running as root, and, while I doubt HIDClient is exfiltrating your passwords, that is an additional surface for bugs or vulnerabilities), or did not support my device(an iPad), and most did not seem to integrate well with my desired control method(using a simple keyboard shortcut/macro keyboard to switch between devices). Additionally, well I did find several projects utilizing a micro controller as a BT HID device, those tended to be more oriented towards automation instead of pass through, and why should I have to purchase an entire micro controller when my PC already has perfectly good Bluetooth support? Remote control software is also a possibility, but then you are subject to the whims of the developers and if they even support your device,  which, for many use cases, they likely do not(consoles, iPad, etc). As such, I decided I would try to address these problems by developing my own software to do Bluetooth HID emulation, and Bluekey is the result of that. 
