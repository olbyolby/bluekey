// HID Report descriptor for updates sent to the host, as per the following format.
// Report structure: A standard 6KRO(up to 6 keys + modifiers at once)
//   Modifier keys: 8 bits, LCtl, LShift, LAlt, LGui, RCtl, RShift, RAlt, RGui; 1 = down, 0 = up
//   Buffer       : 1 byte, constant; 0x00
//   Key 1        : 1 byte, key code; 0x00 = No keydown, see "Keyboard Page" of HID usage tables(pg 89) for more information
//   key 2        :    .  ,    .    ;  .
//   key 3        :    .  ,    .    ;  .
//   key 4        :    .       .    ;  .
//   key 5        :    .  ,    .    ;  .
//   key 6        :    .  ,    .    ;  .
// LED structure: Sent by host to update keyboard's LEDs
//  LED states    : 5 bits, Num lock, Caps Lock, SCroll Lock, Compose, Kana; 1 = on, 0 = off, see "LED Page" of HID usage tables(pg 97)
//  Buffer        : 3 bits, constant; 000  ^^^ 1 byte total
pub(super) const REPORT_DESCRIPTOR: &'static [u8] = &[
    0x05, 0x01,        // Usage Page (Generic Desktop Ctrls)
    0x09, 0x06,        // Usage (Keyboard)
    0xA1, 0x01,        // Collection (Application)
    0x05, 0x07,        //   Usage Page (Kbrd/Keypad)
    0x19, 0xE0,        //   Usage Minimum (0xE0)
    0x29, 0xE7,        //   Usage Maximum (0xE7)
    0x15, 0x00,        //   Logical Minimum (0)
    0x25, 0x01,        //   Logical Maximum (1)
    0x75, 0x01,        //   Report Size (1)
    0x95, 0x08,        //   Report Count (8)
    0x81, 0x02,        //   Input (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position)
    0x95, 0x01,        //   Report Count (1)
    0x75, 0x08,        //   Report Size (8)
    0x81, 0x01,        //   Input (Const,Array,Abs,No Wrap,Linear,Preferred State,No Null Position)
    0x95, 0x05,        //   Report Count (5)
    0x75, 0x01,        //   Report Size (1)
    0x05, 0x08,        //   Usage Page (LEDs)
    0x19, 0x01,        //   Usage Minimum (Num Lock)
    0x29, 0x05,        //   Usage Maximum (Kana)
    0x91, 0x02,        //   Output (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position,Non-volatile)
    0x95, 0x01,        //   Report Count (1)
    0x75, 0x03,        //   Report Size (3)
    0x91, 0x01,        //   Output (Const,Array,Abs,No Wrap,Linear,Preferred State,No Null Position,Non-volatile)
    0x95, 0x06,        //   Report Count (6)
    0x75, 0x08,        //   Report Size (8)
    0x15, 0x00,        //   Logical Minimum (0)
    0x25, 0x65,        //   Logical Maximum (101)
    0x05, 0x07,        //   Usage Page (Kbrd/Keypad)
    0x19, 0x00,        //   Usage Minimum (0x00)
    0x29, 0x65,        //   Usage Maximum (0x65)
    0x81, 0x00,        //   Input (Data,Array,Abs,No Wrap,Linear,Preferred State,No Null Position)
    0xC0,              // End Collection
];