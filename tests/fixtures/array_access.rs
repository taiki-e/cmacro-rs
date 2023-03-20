#[doc(hidden)]
#[macro_export] macro_rules! __cmacro___REENT_INIT_PTR_ZEROED {
  ($var:expr) => {{
    {
      (*$var)._stdin = ptr::addr_of_mut!(*__sf.offset(0));
      (*$var)._stdin
    };
    {
      (*$var)._stdout = ptr::addr_of_mut!(*__sf.offset(1));
      (*$var)._stdout
    };
    {
      (*$var)._stderr = ptr::addr_of_mut!(*__sf.offset(2));
      (*$var)._stderr
    };
  }};
}
pub use __cmacro___REENT_INIT_PTR_ZEROED as _REENT_INIT_PTR_ZEROED;

#[doc(hidden)]
#[macro_export]
macro_rules! __cmacro__ARRAY_ACCESS {
  ($a:expr) => {
    * $a.offset(0)
  };
}
pub use __cmacro__ARRAY_ACCESS as ARRAY_ACCESS;

#[doc(hidden)]
#[macro_export]
macro_rules! __cmacro__ARRAY_FIELD_ACCESS {
  ($a:expr) => {
    (* $a.offset(0)).field
  };
}
pub use __cmacro__ARRAY_FIELD_ACCESS as ARRAY_FIELD_ACCESS;

#[doc(hidden)]
#[macro_export]
macro_rules! __cmacro__FIELD_ARRAY_ACCESS {
  ($a:expr) => {
    * $a.field.offset(0)
  };
}
pub use __cmacro__FIELD_ARRAY_ACCESS as FIELD_ARRAY_ACCESS;


#[doc (hidden)]
#[macro_export]
macro_rules! __cmacro__NESTED_ARRAY_ACCESS {
  ($a:expr) => {
    * (* $a.offset(0)).offset(0)
  };
}
pub use __cmacro__NESTED_ARRAY_ACCESS as NESTED_ARRAY_ACCESS ;
