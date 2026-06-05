use std::cell::{Cell, RefCell};

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::TagKind::{EndTag, StartTag};
use html5ever::tokenizer::{
    BufferQueue, CharacterTokens, TagToken, Token, TokenSink, TokenSinkResult, Tokenizer,
    TokenizerOpts,
};

struct TextExtractor {
    text: RefCell<String>,
    skip_depth: Cell<u32>,
}

impl TokenSink for TextExtractor {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        match token {
            CharacterTokens(s) => {
                if self.skip_depth.get() == 0 {
                    self.text.borrow_mut().push_str(&s);
                }
            }
            TagToken(tag) => {
                if &*tag.name == "script" || &*tag.name == "style" {
                    match tag.kind {
                        StartTag => self.skip_depth.set(self.skip_depth.get() + 1),
                        EndTag => self.skip_depth.set(self.skip_depth.get().saturating_sub(1)),
                    }
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

pub(crate) fn strip_html_tags(html: &str) -> String {
    let sink = TextExtractor {
        text: RefCell::new(String::new()),
        skip_depth: Cell::new(0),
    };

    let input = BufferQueue::default();
    input.push_back(StrTendril::from(html));

    let tok = Tokenizer::new(sink, TokenizerOpts::default());
    let _ = tok.feed(&input);
    tok.end();

    let text = tok.sink.text.borrow().clone();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
