use crate::Message;
use std::collections::BTreeSet;

#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct Filter {
    pub ttaaii: BTreeSet<String>,
    pub cccc: BTreeSet<String>,
    pub awips_id: BTreeSet<String>,
}

impl Filter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_ttaaii<I: IntoIterator<Item = S>, S: Into<String>>(self, ttaaii: I) -> Self {
        Self {
            ttaaii: ttaaii.into_iter().map(|s| s.into()).collect(),
            cccc: self.cccc,
            awips_id: self.awips_id,
        }
    }

    pub fn with_cccc<I: IntoIterator<Item = S>, S: Into<String>>(self, cccc: I) -> Self {
        Self {
            ttaaii: self.ttaaii,
            cccc: cccc.into_iter().map(|s| s.into()).collect(),
            awips_id: self.awips_id,
        }
    }

    pub fn with_awips_id<I: IntoIterator<Item = S>, S: Into<String>>(self, awips_id: I) -> Self {
        Self {
            ttaaii: self.ttaaii,
            cccc: self.cccc,
            awips_id: awips_id.into_iter().map(|s| s.into()).collect(),
        }
    }

    /// Does this filter match an item?
    ///
    /// # Example
    ///
    /// ```
    /// let filter = nwws_http::Filter::new().with_cccc(Some("bar"));
    ///
    /// assert_eq!(true, filter.matches(nwws_http::FilterItem{
    ///     ttaaii: "foo",
    ///     cccc: "bar",
    ///     awips_id: None,
    /// }));
    ///
    /// assert_eq!(false, filter.matches(nwws_http::FilterItem{
    ///     ttaaii: "foo",
    ///     cccc: "baz",
    ///     awips_id: None,
    /// }));
    /// ```
    pub fn matches<'a, I: Into<FilterItem<'a>>>(&'a self, item: I) -> bool {
        let query = item.into();

        (if self.ttaaii.is_empty() {
            true
        } else {
            self.ttaaii.contains(query.ttaaii)
        }) && (if self.cccc.is_empty() {
            true
        } else {
            self.cccc.contains(query.cccc)
        }) && (if self.awips_id.is_empty() {
            true
        } else {
            query
                .awips_id
                .map(|id| self.awips_id.contains(id))
                .unwrap_or(false)
        })
    }
}

impl From<&hyper::Uri> for Filter {
    fn from(uri: &hyper::Uri) -> Self {
        let mut filter = Filter::new();
        if let Some(q) = uri.query() {
            let params = url::form_urlencoded::parse(q.as_bytes());
            for (key, value) in params {
                filter = match key.as_ref() {
                    "ttaaii" => filter.with_ttaaii(value.split(",")),
                    "cccc" => filter.with_cccc(value.split(",")),
                    "awips_id" => filter.with_awips_id(value.split(",")),
                    _ => filter,
                }
            }
        }
        filter
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct FilterItem<'a> {
    pub ttaaii: &'a str,
    pub cccc: &'a str,
    pub awips_id: Option<&'a str>,
}

impl<'a> From<&'a Message> for FilterItem<'a> {
    fn from(m: &'a Message) -> Self {
        Self {
            ttaaii: &m.ttaaii,
            cccc: &m.cccc,
            awips_id: m.awips_id.as_ref().map(String::as_str),
        }
    }
}
