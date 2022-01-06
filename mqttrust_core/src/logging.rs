#[cfg(feature = "log")]
#[macro_export]
macro_rules! mqtt_log {
    ($level:ident, $($arg:expr),*) => {
        log::$level!($($arg),*);
    }
}
#[cfg(all(feature = "defmt", not(feature = "log")))]
#[macro_export]
macro_rules! mqtt_log {
    ($level:ident, $($arg:expr),*) => {
        defmt::$level!($($arg),*);
    }
}
#[cfg(not(any(feature = "defmt", feature = "log")))]
#[macro_export]
macro_rules! mqtt_log {
    ($level:ident, $($arg:expr),*) => {
        {
            $( let _ = $arg; )*
        }

    }
}
#[cfg(all(feature = "defmt", feature = "log"))]
compile_error!("You must enable at most one of the following features: defmt-*, log");
