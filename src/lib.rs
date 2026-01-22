// #![cfg_attr(not(test), no_std)]
#![allow(async_fn_in_trait)]

use heapless::String;

mod input;
mod utf8;

use input::Parser;

/// Zero-allocation handler trait using a GAT for the returned future.
/// The returned future’s lifetime `'a` is tied to the borrowed `Context`/`Level`.
pub trait Handle<W: embedded_io_async::Write> {
    async fn handle<'a>(&self, ctx: &mut Context<'a, W>, level: Level<'a>) -> Result<(), W::Error>;
}

pub async fn run<
    R: embedded_io_async::Read,
    W: embedded_io_async::Write<Error = R::Error>,
    H: Handle<W>,
>(
    reader: &mut R,
    writer: &mut W,
    handle: H,
) -> Result<(), R::Error> {
    let mut buf = [0; 64];
    let mut parser = Parser::new();

    // currently entered line
    let mut line: String<64> = String::new();

    loop {
        let n = reader.read(&mut buf).await?;

        for &byte in &buf[..n] {
            let action = parser.advance(byte);

            use input::{Action as A, ControlCharacter as CC};

            match action {
                // exit CLI
                A::ControlCharacter(CC::CtrlC | CC::CtrlD) if line.is_empty() => {
                    return Ok(());
                }
                // clear line
                A::ControlCharacter(CC::CtrlC) => {
                    // carriage return, then clear after cursor
                    writer.write_all(b"\r\x1B[0K> ").await?;
                    line.clear();
                }
                // write
                A::Print(utf8_char) => {
                    let utf8_char = unsafe { core::str::from_utf8_unchecked(utf8_char.as_bytes()) };
                    if let Ok(_) = line.push_str(utf8_char) {
                        writer.write_all(utf8_char.as_bytes()).await?;
                    }
                }
                // backspace
                A::ControlCharacter(CC::Backspace | CC::CtrlH) => {
                    if let Some(_) = line.pop() {
                        // backspace + delete after cursor
                        writer.write_all(b"\x08\x1B[0K").await?;
                    }
                }
                // enter
                A::ControlCharacter(CC::CarriageReturn) => {
                    let level = Level::new(&line);
                    let mut ctx_type = ContextType::Execute;
                    let mut ctx = Context::new(&mut ctx_type, writer);
                    handle.handle(&mut ctx, level).await?;
                    line.clear();
                    writer.write_all(b"\r\n> ").await?;
                }
                // autocomplete
                A::ControlCharacter(CC::Tab) => {
                    // split off last part of command ("_" is whitespace)
                    //
                    // "cmd1" no whitespace at end, try to complete cmd1
                    // rsplit_once returns None -> to_complete = line
                    //
                    // "cmd1_" whitespace at end, try to complete next command
                    // rsplit_once returns Some("cmd1", "") -> cmd_path = "cmd1", to_complete = ""

                    let (command_path, to_complete) = match line.rsplit_once(char::is_whitespace) {
                        // no whitespace ->
                        None => ("", line.as_str()),
                        Some(x) => x,
                    };

                    // first try to complete the last word in-line, eg "con" -> "config"
                    // if that fails, eg. because there is also "connection", print all available options
                    let level = Level::new(command_path);
                    let mut autocomplete_best_match = None;
                    let mut exact_match = false;
                    let mut ctx_type = ContextType::AutocompleteBestMatch {
                        autocomplete_best_match: &mut autocomplete_best_match,
                        exact_match: &mut exact_match,
                        to_complete,
                    };

                    let mut ctx = Context::new(&mut ctx_type, writer);
                    handle.handle(&mut ctx, level).await?;

                    match autocomplete_best_match {
                        // no completion found
                        None => (),
                        // multiple matches diverge at the first character
                        // this should always happen after pressing tab twice
                        // list all matching completions
                        Some("") => {
                            let mut ctx_type = ContextType::AutocompleteList(to_complete);
                            let mut ctx = Context::new(&mut ctx_type, writer);
                            handle.handle(&mut ctx, level).await?;
                            if ctx.printed_stuff {
                                writer.write_all(b"\r\n> ").await?;
                                writer.write_all(line.as_bytes()).await?;
                            }
                        }
                        // non-empty match found -> can append completion to line
                        Some(complete) => {
                            if let Ok(_) = line.push_str(complete) {
                                writer.write_all(complete.as_bytes()).await?;

                                if exact_match {
                                    if let Ok(_) = line.push_str(" ") {
                                        writer.write_all(b" ").await?;
                                    }
                                }
                            }
                        }
                    }
                }
                _ => (),
            }
        }

        writer.flush().await?;
    }
}

#[derive(PartialEq)]
enum ContextType<'a> {
    Execute,
    AutocompleteBestMatch {
        autocomplete_best_match: &'a mut Option<&'static str>,
        exact_match: &'a mut bool,
        to_complete: &'a str,
    },
    AutocompleteList(&'a str),
}

#[derive(Debug, Copy, Clone)]
pub struct Level<'a> {
    line: &'a str,
}

impl<'a> Level<'a> {
    fn new(line: &'a str) -> Self {
        Self { line }
    }
}

pub struct Context<'a, W: embedded_io_async::Write> {
    ctx_type: &'a mut ContextType<'a>,
    printed_stuff: bool,
    done: bool,
    writer: &'a mut W,
}

impl<'a, W: embedded_io_async::Write> Context<'a, W> {
    fn new(ctx_type: &'a mut ContextType<'a>, writer: &'a mut W) -> Self {
        Self {
            ctx_type,
            printed_stuff: false,
            done: false,
            writer,
        }
    }

    pub fn exec(&self, level: Level<'_>) -> bool {
        if self.done {
            return false;
        }
        // todo: raise error when not empty
        *self.ctx_type == ContextType::Execute && level.line.is_empty()
    }

    pub fn exec_arg<'l>(&self, level: Level<'l>) -> Option<&'l str> {
        if self.done {
            return None;
        }

        if *self.ctx_type == ContextType::Execute {
            let line = level.line.trim_end();
            if line.is_empty() {
                // TODO: raise error
                None
            } else {
                Some(line)
            }
        } else {
            None
        }
    }

    pub async fn command<'l>(
        &mut self,
        level: Level<'l>,
        name: &'static str,
    ) -> Result<Option<Level<'l>>, W::Error> {
        if self.done {
            return Ok(None);
        }

        // line empty -> reached target level, do autocomplete
        // otherwise try descending command structure
        if !level.line.is_empty() {
            // check if the command matches exactly
            if let Some(prefix_removed) = level.line.strip_prefix(name) {
                // only descend if either whitespace or EOL follow
                // prevent matching if one command is a prefix of another one
                // config would otherwise also match the input "configuration"
                if prefix_removed
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace)
                {
                    return Ok(Some(Level {
                        line: prefix_removed.trim_start(),
                    }));
                }
            }
        } else {
            self.hint_autocomplete(name).await?;
        }

        Ok(None)
    }

    pub async fn print(&mut self, data: impl AsRef<[u8]>) -> Result<(), W::Error> {
        self._print(data.as_ref()).await
    }

    async fn _print(&mut self, data: &[u8]) -> Result<(), W::Error> {
        // clear current line first, will be printed again by cli
        if !self.printed_stuff {
            self.printed_stuff = true;
            if *self.ctx_type == ContextType::Execute {
                self.writer.write_all(b"\r\n").await?; // crlf -> keep input line
            } else {
                self.writer.write_all(b"\r\x1B[0K").await?; // carriage return, then clear after cursor
            }
        }
        self.writer.write_all(data.as_ref()).await
    }

    pub async fn hint_autocomplete(&mut self, name: &'static str) -> Result<(), W::Error> {
        // check if the command can be autocompleted
        match self.ctx_type {
            ContextType::AutocompleteBestMatch {
                autocomplete_best_match,
                exact_match,
                to_complete,
            } => {
                let x = name.strip_prefix(*to_complete);

                if let Some(rest) = x {
                    if let Some(prev_match) = autocomplete_best_match {
                        **exact_match = false;
                        **autocomplete_best_match = Some(common_prefix(prev_match, rest));
                    } else {
                        **exact_match = true;
                        **autocomplete_best_match = Some(rest);
                    }
                }
            }
            ContextType::AutocompleteList(to_complete) => {
                if name.starts_with(*to_complete) {
                    if self.printed_stuff {
                        self.print(b" ").await?;
                    }
                    self.print(name).await?;
                }
            }
            ContextType::Execute => (),
        }

        Ok(())
    }
}

fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let mut end = 0;

    for (ca, cb) in a.chars().zip(b.chars()) {
        if ca != cb {
            break;
        }
        end += ca.len_utf8();
    }

    &a[..end]
}
