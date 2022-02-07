use std::pin::Pin;
use std::task::{Context, Poll};

pub struct NwwsOiStream(nwws_oi::Stream);
impl std::fmt::Debug for NwwsOiStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NwwsOiStream").finish_non_exhaustive()
    }
}

impl NwwsOiStream {
    pub fn new<C: Into<nwws_oi::Config>>(config: C) -> Self {
        nwws_oi::Stream::new(config).into()
    }
}

impl From<nwws_oi::Stream> for NwwsOiStream {
    fn from(inner: nwws_oi::Stream) -> Self {
        Self(inner)
    }
}

impl futures::Stream for NwwsOiStream {
    type Item = Result<nwws_oi::Message, nwws_oi::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.0).poll_next(cx) {
                Poll::Ready(Some(nwws_oi::StreamEvent::Message(m))) => {
                    return Poll::Ready(Some(Ok(m)))
                }
                Poll::Ready(Some(nwws_oi::StreamEvent::Error(e))) => {
                    return Poll::Ready(Some(Err(e)))
                }
                Poll::Ready(Some(nwws_oi::StreamEvent::ConnectionState(_))) => continue,
                Poll::Ready(None) => unreachable!("nwws_oi::Stream ended"),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl From<nwws_oi::Message> for crate::Message {
    fn from(v: nwws_oi::Message) -> Self {
        Self {
            ttaaii: v.ttaaii,
            cccc: v.cccc,
            awips_id: v.awips_id,
            issue: v.issue,
            nwws_oi_id: Some(v.id),
            nwws_oi_delay_stamp: v.delay_stamp,
            message: v.message,
        }
    }
}

impl TryFrom<Result<nwws_oi::Message, nwws_oi::Error>> for crate::Message {
    type Error = nwws_oi::Error;

    fn try_from(value: Result<nwws_oi::Message, nwws_oi::Error>) -> Result<Self, Self::Error> {
        value.map(crate::Message::from)
    }
}
