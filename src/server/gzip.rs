use bytes::Bytes;
use miniz_oxide::deflate::core::{CompressorOxide, TDEFLFlush, TDEFLStatus};
use miniz_oxide::deflate::CompressionLevel;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Debug, Clone)]
pub enum Block {
    /// A block immediately after a sync point
    First(Bytes),
    /// A block in the middle of a stream
    Middle(Bytes),
}

impl AsRef<Bytes> for Block {
    fn as_ref(&self) -> &Bytes {
        match self {
            Block::First(b) => b,
            Block::Middle(b) => b,
        }
    }
}

impl From<Block> for Bytes {
    fn from(b: Block) -> Self {
        match b {
            Block::First(b) => b,
            Block::Middle(b) => b,
        }
    }
}

static GZIP_HEADER: &[u8] = [
    0x1f, 0x8b, // gzip magic
    8,    // deflate
    0,    // flags
    0, 0, 0, 0,   // mtime
    0,   // extra flags
    255, // unknown OS
]
.as_slice();

pub struct Transform {
    wrote_header: bool,
    deflate: CompressorOxide,
    state_just_flushed: bool,
}

impl Transform {
    pub fn new(level: CompressionLevel) -> Self {
        let mut deflate = CompressorOxide::new(0);
        deflate.set_compression_level(level);

        Self {
            wrote_header: false,
            deflate,
            state_just_flushed: true,
        }
    }

    pub fn write_block(&mut self, input: &[u8], flush_state: bool) -> Result<Block, TDEFLStatus> {
        let mut output = Vec::with_capacity(input.len() * 2 / 7 + 20);

        if !self.wrote_header {
            self.wrote_header = true;
            output.extend_from_slice(GZIP_HEADER);
        }

        self.write(
            input,
            &mut output,
            if flush_state {
                TDEFLFlush::Full
            } else {
                TDEFLFlush::Sync
            },
        )?;

        output.shrink_to_fit();
        let output = Bytes::from(output);

        log::trace!("compressed {} -> {}", input.len(), output.len());

        let block = if self.state_just_flushed {
            Block::First(output)
        } else {
            Block::Middle(output)
        };
        self.state_just_flushed = flush_state;

        Ok(block)
    }

    fn write(
        &mut self,
        input: &[u8],
        output: &mut Vec<u8>,
        flush: TDEFLFlush,
    ) -> Result<(), TDEFLStatus> {
        let mut in_pos = 0;

        loop {
            let (status, consumed) = miniz_oxide::deflate::core::compress_to_output(
                &mut self.deflate,
                &input[in_pos..],
                flush,
                |slice| {
                    output.extend_from_slice(slice);
                    true
                },
            );
            match status {
                TDEFLStatus::Okay | TDEFLStatus::Done => {
                    in_pos += consumed;
                    if in_pos == input.len() {
                        return Ok(());
                    }
                }
                err => return Err(err),
            }
        }
    }
}

pub struct Stream<S> {
    inner: Pin<Box<S>>,
    transform: Transform,
}

impl<S: futures::Stream<Item = hyper::http::Result<Bytes>>> Stream<S> {
    pub fn new(inner: S, level: CompressionLevel) -> Self {
        log::trace!("new gzip stream");
        Self {
            inner: Box::pin(inner),
            transform: Transform::new(level),
        }
    }
}

impl<S: futures::Stream<Item = hyper::http::Result<Bytes>>> futures::Stream for Stream<S> {
    type Item = hyper::http::Result<Block>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let bytes = match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(bytes))) => bytes,
            Poll::Ready(None) => {
                // TODO: CRC32, isize
                return Poll::Ready(None);
            }
            Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
            Poll::Pending => return Poll::Pending,
        };

        match self.transform.write_block(&bytes, false) {
            Ok(block) => Poll::Ready(Some(Ok(block))),
            Err(_) => Poll::Ready(None),
        }
    }
}
