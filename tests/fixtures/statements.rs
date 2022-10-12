#[doc(hidden)]
#[macro_export]
macro_rules! __cmacro__vTaskDelayUntil {
  ($pxPreviousWakeTime:expr, $xTimeIncrement:expr) => {
    loop {
      {
        drop(xTaskDelayUntil(($pxPreviousWakeTime).into(), ($xTimeIncrement).into()))
      };

      if 0 == Default::default() {
        break
      }
    }
  };
}
pub use __cmacro__vTaskDelayUntil as vTaskDelayUntil;

pub const pdFALSE: _ = 0;

#[doc(hidden)]
#[macro_export]
macro_rules! __cmacro__portEND_SWITCHING_ISR {
  ($xSwitchRequired:expr) => {
    if $xSwitchRequired != 0 {
      vPortYield();
    }
  };
}
pub use __cmacro__portEND_SWITCHING_ISR as portEND_SWITCHING_ISR;

pub const JSVAL_TAG_MAX_DOUBLE: uint32_t = 131056 as uint32_t;

pub const JSVAL_TAG_SHIFT: _ = 47;

#[doc(hidden)]
#[macro_export]
macro_rules! __cmacro__JSVAL_TYPE_TO_TAG {
  ($type:expr) => {
    (131056 as uint32_t | $type) as JSValueTag
  };
}
pub use __cmacro__JSVAL_TYPE_TO_TAG as JSVAL_TYPE_TO_TAG;

#[doc(hidden)]
#[macro_export]
macro_rules! __cmacro__JSVAL_TYPE_TO_SHIFTED_TAG {
  ($type:expr) => {
    (131056 as uint32_t | $type) as JSValueTag as uint64_t << 47
  };
}
pub use __cmacro__JSVAL_TYPE_TO_SHIFTED_TAG as JSVAL_TYPE_TO_SHIFTED_TAG;
