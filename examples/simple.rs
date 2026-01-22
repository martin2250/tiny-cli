use std::io::{self, Read, Stdin, Stdout, Write};
use tiny_cli::{Context, Handle, Level};

pub struct StdinReader(pub Stdin);

impl embedded_io_async::ErrorType for StdinReader {
    type Error = std::io::Error;
}

impl embedded_io_async::Read for StdinReader {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.0.read(buf)
    }
}

pub struct StdoutWriter(pub Stdout);

impl embedded_io_async::ErrorType for StdoutWriter {
    type Error = std::io::Error;
}

impl embedded_io_async::Write for StdoutWriter {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.0.write(buf)
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        self.0.flush()
    }
}

// Concrete handler type (zero-sized)
struct MyHandler;

// Implement the zero-allocation handler trait via a GAT.
// The associated future is the one produced by the async fn `handle_cli`.
impl<W: embedded_io_async::Write> Handle<W> for MyHandler {
    async fn handle<'a>(&self, ctx: &mut Context<'a, W>, level: Level<'a>) -> Result<(), W::Error> {
        handle_cli(ctx, level).await
    }
}

fn main() {
    smol::block_on(async {
        crossterm::terminal::enable_raw_mode().unwrap();
        let stdout = io::stdout();
        let stdin = io::stdin();

        // we should leave this up to the implementation tbh
        // let mut writer_buf = buffered_io::asynch::BufferedWrite::new(writer);

        tiny_cli::run(
            &mut StdinReader(stdin),
            &mut StdoutWriter(stdout),
            MyHandler,
        )
        .await
        .unwrap();
    });
}

async fn handle_cli<'a, 'b, W: embedded_io_async::Write>(
    ctx: &'a mut Context<'b, W>,
    level: Level<'b>,
) -> Result<(), W::Error> {
    // nested levels and exec / exec_arg
    if let Some(level) = ctx.command(level, "config").await? {
        for name in ["enable", "logging", "logfile", "connetion", "constant"] {
            if let Some(level) = ctx.command(level, name).await? {
                // config items get/set
                if let Some(level) = ctx.command(level, "set").await? {
                    if let Some(arg) = ctx.exec_arg(level) {
                        ctx.print(format!("set value {} {}", name, arg)).await?;
                    }
                }
                if ctx.exec(level) {
                    ctx.print(format!("get value {}", name)).await?;
                }
            }
        }
    }

    // test hint_autocomplete
    if let Some(level) = ctx.command(level, "fruit").await? {
        if let Some(arg) = ctx.exec_arg(level) {
            ctx.print(format!("set fruit {}", arg)).await?;
        } else {
            ctx.hint_autocomplete("apple").await?;
            ctx.hint_autocomplete("apricot").await?;
            ctx.hint_autocomplete("banana").await?;
            ctx.hint_autocomplete("strawberry").await?;
        }
    }

    if let Some(level) = ctx.command(level, "save").await? {
        if ctx.exec(level) {
            ctx.print(format!("save")).await?;
        }
    }

    if let Some(level) = ctx.command(level, "reboot").await? {
        if ctx.exec(level) {
            ctx.print(format!("reboot")).await?;
        }
    }

    // don't do this :S
    if let Some(mut level) = ctx.command(level, "infinite").await? {
        let mut n = 0;
        while let Some(new_level) = ctx.command(level, "loop").await? {
            level = new_level;
            n += 1;
            if ctx.exec(level) {
                ctx.print(format!("reached level {}", n)).await?;
            }
        }
    }

    Ok(())
}
