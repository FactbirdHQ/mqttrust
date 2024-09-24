use core::str::FromStr;

use itertools::{EitherOrBoth, Itertools};

use crate::{crate_config::MAX_TOPIC_LEN, Error};

/// A topic filter.
///
/// An MQTT topic filter is a multi-field string, delimited by forward
/// slashes, '/', in which fields can contain the wildcards:
///     '+' - Matches a single field
///     '#' - Matches all subsequent fields (must be last field in filter)
///
/// It can be used to match against topics.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TopicFilter {
    filter: heapless::String<{ MAX_TOPIC_LEN }>,
    has_wildcards: bool,
}

impl TopicFilter {
    /// Creates a new topic filter from the string.
    /// This can fail if the filter is not correct, such as having a '#'
    /// wildcard in anyplace other than the last field, or if
    pub fn new(filter: &str) -> Result<Self, Error> {
        let filter =
            heapless::String::<{ MAX_TOPIC_LEN }>::from_str(filter).map_err(|_| Error::Overflow)?;
        let n = filter.len();

        if n == 0 {
            return Err(Error::BadTopicFilter);
        }

        // If the topic contains any wildcards.
        let has_wildcards = match filter.find('#') {
            Some(i) if i < n - 1 => return Err(Error::BadTopicFilter),
            Some(_) => true,
            None => filter.contains('+'),
        };

        Ok(Self {
            filter,
            has_wildcards,
        })
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// Determines if the topic matches the filter.
    pub fn is_match(&self, topic: &str) -> bool {
        if self.has_wildcards {
            for matcher in self.filter.split('/').zip_longest(topic.split('/')) {
                match matcher {
                    EitherOrBoth::Both(filter, field) => {
                        if filter == "#" {
                            break;
                        } else if filter != "+" && filter != field {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }

            true
        } else {
            self.filter == topic
        }
    }
}

impl core::fmt::Display for TopicFilter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.filter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nonwild_topic_filter() {
        const FILTER: &str = "some/topic";

        let filter = TopicFilter::new(FILTER).unwrap();
        assert!(filter.is_match(FILTER));

        assert_eq!(filter.filter(), FILTER);
    }

    #[test]
    fn test_topic_filter() {
        const FILTER1: &str = "some/topic/#";

        let filter = TopicFilter::new(FILTER1).unwrap();
        assert!(filter.is_match("some/topic/thing"));
        assert!(filter.is_match("some/topic/thing/more/sections"));

        assert_eq!(filter.filter(), FILTER1);

        const FILTER2: &str = "some/+/thing";
        let filter = TopicFilter::new(FILTER2).unwrap();
        assert!(filter.is_match("some/topic/thing"));
        assert!(!filter.is_match("some/topic/plus/thing"));

        assert_eq!(filter.filter(), FILTER2);

        const FILTER3: &str = "some/+";
        let filter = TopicFilter::new(FILTER3).unwrap();
        assert!(filter.is_match("some/thing"));
        assert!(!filter.is_match("some/thing/plus"));
    }
}
