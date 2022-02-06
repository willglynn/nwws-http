use serde::{Deserialize, Serialize};

/// A message received from NWWS.
///
/// See the [NWS Communications Header Policy Document](https://www.weather.gov/tg/awips) for
/// information about how to interpret this data.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// The six character WMO product ID
    pub ttaaii: String,

    /// Four character issuing center
    pub cccc: String,

    /// The six character AWIPS ID, sometimes called AFOS PIL
    pub awips_id: Option<String>,

    /// The time at which this product was issued
    pub issue: chrono::DateTime<chrono::FixedOffset>,

    /// A short-term-unique ID for this message, if received from NWWS-OI
    ///
    /// The id contains two numbers separated by a period. The first number is the UNIX process ID
    /// on the system running the ingest process. The second number is a simple incremented
    /// sequence number for the product. Gaps in the sequence likely indicate message loss.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nwws_oi_id: Option<String>,

    /// The time at which the message was originally sent by the NWS ingest process to the NWWS-OI
    /// XMPP server, if it differs substantially from the current time.
    ///
    /// See [XEP-0203](https://xmpp.org/extensions/xep-0203.html) for more details.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub nwws_oi_delay_stamp: Option<chrono::DateTime<chrono::FixedOffset>>,

    /// The contents of the message
    pub message: String,
}
