macro_rules! make_table {
    ($name:ident {$($id:ident = $value:literal),*}) => {
        #[derive(Clone, Copy, Debug)]
        pub enum $name {
            $($id),*
        }

        impl $name {
            pub fn from_id(id: u8) -> Result<Self, InvalidId> {
                match id {
                    $($value => Ok(Self::$id)),*,
                    _ => Err(InvalidId)
                }
            }
            pub fn into_id(&self) -> u8 {
                match self {
                    $(Self::$id => $value),*
                }
            }
        }
        impl Into<u8> for $name {
            fn into(self) -> u8 {
                self.into_id()
            }
        }

        impl TryFrom<u8> for $name {
            type Error = InvalidId;
            fn try_from(value: u8) -> Result<Self, InvalidId> {
                Self::from_id(value)
            }
        }
    }
}

// Big list of every keyboard LED supported by the USB standard
// See HID usage tables "LEDs"(pg 97)
#[derive(Clone, Copy, Debug)]
pub struct InvalidId;
make_table!(Led {
    NumLock = 0x01,
    CapsLock = 0x02,
    ScrollLock = 0x03,
    Compose = 0x04,
    Kana = 0x05,
    Power = 0x06,
    Shift = 0x07,
    DoNotDisturb = 0x08,
    Mute = 0x09,
    ToneEnable = 0x0A,
    HighCutFilter = 0x0B,
    LowCutFilter = 0x0C,
    EqualizerEnable = 0x0D,
    SoundFieldOn = 0x0E,
    SurroundOn = 0x0F,
    Repeat = 0x10,
    Stereo = 0x11,
    SamplingRateDetect = 0x12,
    Spinning = 0x13,
    Cav = 0x14,
    Clv = 0x15,
    RecordingFormatDetect = 0x16,
    OffHook = 0x17,
    Ring = 0x18,
    MessageWaiting = 0x19,
    DataMode = 0x1A,
    BatteryOperation = 0x1B,
    BatteryOk = 0x1C,
    BatteryLow = 0x1D,
    Speaker = 0x1E,
    Headset = 0x1F,
    Hold = 0x20,
    Microphone = 0x21,
    Coverage = 0x22,
    NightMode = 0x23,
    SendCalls = 0x24,
    CallPickup = 0x25,
    Conference = 0x26,
    StandBy = 0x27,
    CameraOn = 0x28,
    CameraOff = 0x29,
    OnLine = 0x2A,
    OffLine = 0x2B,
    Busy = 0x2C,
    Ready = 0x2D,
    PaperOut = 0x2E,
    PaperJam = 0x2F,
    Remote = 0x30,
    Forward = 0x31,
    Reverse = 0x32,
    Stop = 0x33,
    Rewind = 0x34,
    FastForward = 0x35,
    Play = 0x36,
    Pause = 0x37,
    Record = 0x38,
    Error = 0x39,
    UsageSelectedIndicator = 0x3A,
    UsageInUseIndicator = 0x3B,
    UsageMultiModeIndicator = 0x3C,
    IndicatorOn = 0x3D,
    IndicatorFlash = 0x3E,
    IndicatorSlowBlink = 0x3F,
    IndicatorFastBlink = 0x40,
    IndicatorOff = 0x41,
    FlashOnTime = 0x42,
    SlowBlinkOnTime = 0x43,
    SlowBlinkOffTime = 0x44,
    FastBlinkOnTime = 0x45,
    FastBlinkOffTime = 0x46,
    UsageIndicatorColor = 0x47,
    IndicatorRed = 0x48,
    IndicatorGreen = 0x49,
    IndicatorAmber = 0x4A,
    GenericIndicator = 0x4B,
    SystemSuspend = 0x4C,
    ExternalPowerConnected = 0x4D,
    IndicatorBlue = 0x4E,
    IndicatorOrange = 0x4F,
    GoodStatus = 0x50,
    WarningStatus = 0x51,
    RgbLed = 0x52,
    RedLedChannel = 0x53,
    BlueLedChannel = 0x54,
    GreenLedChannel = 0x55,
    LedIntensity = 0x56,
    SystemMicrophoneMute = 0x57,
    PlayerIndicator = 0x60,
    Player1 = 0x61,
    Player2 = 0x62,
    Player3 = 0x63,
    Player4 = 0x64,
    Player5 = 0x65,
    Player6 = 0x66,
    Player7 = 0x67
});

