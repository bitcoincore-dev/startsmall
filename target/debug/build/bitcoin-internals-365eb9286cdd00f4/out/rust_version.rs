/// Expands code based on Rust version this is compiled under.
///
/// Example:
/// ```
/// bitcoin_internals::rust_version! {
///     if >= 1.78 {
///         println!("This is Rust 1.78+");
///     } else {
///         println!("This is Rust < 1.78");
///     }
/// }
/// ```
///
/// The `else` branch is optional.
/// Currently only the `>=` operator is supported.
#[macro_export]
macro_rules! rust_version {
    (if >= 1.74 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.75 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.76 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.77 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.78 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.79 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.80 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.81 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.82 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.83 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.84 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.85 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.86 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.87 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.88 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.89 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.90 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.91 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.92 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.93 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.94 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.95 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.96 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= 1.97 { $($if_yes:tt)* } $(else { $($if_no:tt)* })?) => {
        $($if_yes)*
    };
    (if >= $unknown:tt $($rest:tt)*) => {
        compile_error!(concat!("unknown Rust version ", stringify!($unknown)));
    };
}
pub use rust_version;
