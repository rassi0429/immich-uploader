use vte::{Params, Parser, Perform};

pub struct LineProcessor {
    parser: Parser,
    state: ProcessorState,
}

struct ProcessorState {
    buffer: String,
    completed: Vec<String>,
}

impl Perform for ProcessorState {
    fn print(&mut self, c: char) {
        self.buffer.push(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => {
                let line = std::mem::take(&mut self.buffer);
                self.completed.push(line);
            }
            b'\r' => {
                self.buffer.clear();
            }
            b'\t' => self.buffer.push('\t'),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, action: char) {
        match action {
            'K' => {
                self.buffer.clear();
            }
            'J' => {
                self.buffer.clear();
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}
}

impl LineProcessor {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            state: ProcessorState {
                buffer: String::new(),
                completed: Vec::new(),
            },
        }
    }

    pub fn process(&mut self, bytes: &[u8]) -> Vec<String> {
        self.parser.advance(&mut self.state, bytes);
        std::mem::take(&mut self.state.completed)
    }

    pub fn current_line(&self) -> &str {
        &self.state.buffer
    }
}

impl Default for LineProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newline_completes_line() {
        let mut p = LineProcessor::new();
        let lines = p.process(b"hello\n");
        assert_eq!(lines, vec!["hello".to_string()]);
        assert_eq!(p.current_line(), "");
    }

    #[test]
    fn carriage_return_overwrites() {
        let mut p = LineProcessor::new();
        let _ = p.process(b"progress 50%");
        let _ = p.process(b"\rprogress 100%");
        assert_eq!(p.current_line(), "progress 100%");
    }

    #[test]
    fn ansi_color_stripped() {
        let mut p = LineProcessor::new();
        let lines = p.process(b"\x1b[31mred\x1b[0m\n");
        assert_eq!(lines, vec!["red".to_string()]);
    }

    #[test]
    fn line_clear_csi_k() {
        let mut p = LineProcessor::new();
        let _ = p.process(b"will be cleared");
        let _ = p.process(b"\x1b[2K");
        assert_eq!(p.current_line(), "");
    }
}
