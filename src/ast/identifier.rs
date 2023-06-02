use std::{borrow::Cow, fmt::Debug};

use nom::{
  branch::alt,
  character::complete::{anychar, char, satisfy},
  combinator::{all_consuming, map, map_opt, recognize, verify},
  multi::fold_many0,
  sequence::{pair, preceded},
  IResult,
};

use super::literal::universal_char;

fn is_identifier_start(c: char) -> bool {
  unicode_ident::is_xid_start(c) || c == '_'
}

pub(crate) fn is_identifier(s: &str) -> bool {
  let mut chars = s.chars();

  if let Some(first_character) = chars.next() {
    return is_identifier_start(first_character) && chars.all(unicode_ident::is_xid_continue)
  }

  false
}

/// A literal identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifier<'t> {
  pub(crate) id: Cow<'t, str>,
}

impl<'t> Identifier<'t> {
  /// Get the string representation of this identifier.
  pub fn as_str(&self) -> &str {
    self.id.as_ref()
  }

  pub(crate) fn parse_str(input: &'t str) -> IResult<&'t str, Self> {
    alt((
      map(
        recognize(pair(
          satisfy(is_identifier_start),
          fold_many0(satisfy(unicode_ident::is_xid_continue), || (), |_, _| ()),
        )),
        |s| Self { id: Cow::Borrowed(s) },
      ),
      |token| {
        let mut identifier_char = alt((map_opt(preceded(char('\\'), universal_char), char::from_u32), anychar));

        let (token, start_char) = verify(&mut identifier_char, |c| is_identifier_start(*c))(token)?;

        fold_many0(
          verify(identifier_char, |c| unicode_ident::is_xid_continue(*c)),
          move || Identifier { id: Cow::Owned(String::from(start_char)) },
          |mut id, c| {
            id.id.to_mut().push(c);
            id
          },
        )(token)
      },
    ))(input)
  }

  pub(crate) fn to_static(&self) -> Identifier<'static> {
    Identifier { id: Cow::Owned(self.id.clone().into_owned()) }
  }
}

impl<'t> TryFrom<&'t str> for Identifier<'t> {
  type Error = nom::Err<nom::error::Error<&'t str>>;

  fn try_from(s: &'t str) -> Result<Self, Self::Error> {
    let (_, id) = all_consuming(Self::parse_str)(s)?;
    Ok(id)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  use crate::lit_id;

  #[test]
  fn parse_literal() {
    let id = Identifier::try_from("asdf").unwrap();
    assert_eq!(id, lit_id!(asdf));

    let id = Identifier::try_from("\\u0401").unwrap();
    assert_eq!(id, lit_id!(Ё));

    let id = Identifier::try_from("Δx").unwrap();
    assert_eq!(id, lit_id!(Δx));

    let id = Identifier::try_from("_123").unwrap();
    assert_eq!(id, lit_id!(_123));

    let id = Identifier::try_from("__INT_MAX__").unwrap();
    assert_eq!(id, lit_id!(__INT_MAX__));
  }

  #[test]
  fn parse_wrong() {
    Identifier::try_from("123def").unwrap_err();
  }
}
