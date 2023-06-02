use std::borrow::Cow;

use nom::{
  bytes::complete::{tag, take_until},
  combinator::all_consuming,
  sequence::delimited,
};

/// A macro argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroArg {
  pub(crate) index: usize,
}

impl MacroArg {
  /// Create a new macro argument with the given index.
  pub fn new(index: usize) -> Self {
    Self { index }
  }
}

impl MacroArg {
  pub(crate) fn index(&self) -> usize {
    self.index
  }
}

/// A comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment<'t> {
  pub(crate) comment: Cow<'t, str>,
}

impl<'t> TryFrom<&'t str> for Comment<'t> {
  type Error = nom::Err<nom::error::Error<&'t str>>;

  fn try_from(s: &'t str) -> Result<Self, Self::Error> {
    let (_, comment) = all_consuming(delimited(tag("/*"), take_until("*/"), tag("*/")))(s)?;
    Ok(Self { comment: Cow::Borrowed(comment) })
  }
}
