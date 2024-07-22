use std::fmt;
use std::sync::OnceLock;

use regex_lite::{Regex, RegexBuilder};
use serde::de::Visitor;
use serde::{Deserialize, Deserializer};
use time::format_description::{self, OwnedFormatItem};

#[derive(Debug, Clone, Copy)]
pub struct Duration(std::time::Duration);

impl Duration {
    pub fn from_secs(seconds: u64) -> Self {
        Self(std::time::Duration::from_secs(seconds))
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DurationVisitor;

        impl<'de> Visitor<'de> for DurationVisitor {
            type Value = Duration;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "a duration")
            }

            fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                self.visit_u64(v.try_into().map_err(E::custom)?)
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Duration::from_secs(v))
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                use serde::de::Unexpected;

                static REGEXP: OnceLock<Regex> = OnceLock::new();

                let regexp = REGEXP.get_or_init(|| {
                    RegexBuilder::new(
                        r"
                        ^
                        (?:(?<days>    \d+)d)? \s*
                        (?:(?<hours>   \d+)h)? \s*
                        (?:(?<minutes> \d+)m)? \s*
                        (?:(?<seconds> \d+)s)?
                        $",
                    )
                    .ignore_whitespace(true)
                    .build()
                    .unwrap()
                });
                let Some(captures) = regexp.captures(v) else {
                    return Err(E::invalid_value(Unexpected::Str(v), &"a duration"));
                };

                let parse = |name: &str| {
                    if let Some(s) = captures.name(name).map(|m| m.as_str()) {
                        s.parse::<u64>()
                            .map(Some)
                            .map_err(|e| E::custom(format!("could not parse {name} (`{s}`): {e}")))
                    } else {
                        Ok(None)
                    }
                };

                let days = parse("days")?;
                let hours = parse("hours")?;
                let minutes = parse("minutes")?;
                let seconds = parse("seconds")?;

                if days.is_none() && hours.is_none() && minutes.is_none() && seconds.is_none() {
                    return Err(E::invalid_value(Unexpected::Str(v), &"a duration"));
                }

                days.unwrap_or(0)
                    .checked_mul(24)
                    .and_then(|h| h.checked_add(hours.unwrap_or(0)))
                    .and_then(|h| h.checked_mul(60))
                    .and_then(|m| m.checked_add(minutes.unwrap_or(0)))
                    .and_then(|m| m.checked_mul(60))
                    .and_then(|s| s.checked_add(seconds.unwrap_or(0)))
                    .map(Duration::from_secs)
                    .ok_or_else(|| E::custom(format!("duration `{v}` is too large")))
            }
        }

        deserializer.deserialize_str(DurationVisitor)
    }
}

impl From<std::time::Duration> for Duration {
    fn from(duration: std::time::Duration) -> Self {
        Self(duration)
    }
}

impl From<Duration> for std::time::Duration {
    fn from(duration: Duration) -> Self {
        duration.0
    }
}

#[derive(Debug, Clone)]
pub struct DateTimeFormat(OwnedFormatItem);

impl DateTimeFormat {
    pub fn into_inner(self) -> OwnedFormatItem {
        self.0
    }
}

impl<'de> Deserialize<'de> for DateTimeFormat {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DateTimeFormatVisitor;

        impl<'de> Visitor<'de> for DateTimeFormatVisitor {
            type Value = DateTimeFormat;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                write!(formatter, "a datetime format")
            }

            fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                format_description::parse_owned::<2>(s)
                    .map(DateTimeFormat)
                    .map_err(E::custom)
            }
        }

        deserializer.deserialize_str(DateTimeFormatVisitor)
    }
}
