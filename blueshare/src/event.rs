
/*
macro_rules! combine {
    ($name:ident($($variant:ident $(: $type:ty)?),+)) => {
        enum $name {
            $($variant(branch!($variant$(,$type)?))),+
        }
        $(impl From<branch!($variant$(,$type)?)> for $name {
            fn from(value: branch!($variant$(,$type)?)) -> Self {
                Self::$variant(value)
            }
        })+
    };
}
macro_rules! branch {
    ($variant:ident, $type:ty) => {
        $type
    };
    ($variant:ident) => {
        $variant
    }
}

type Id = u64;
combine!(Tester(
    Number: u16,
    Id
));
*/
