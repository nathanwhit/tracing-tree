use ansi_term::{Color, Style};
use chrono::{DateTime, Local};
use std::sync::Mutex;
use std::{fmt, io, io::Write as _};
use tracing::{
    field::{Field, Visit},
    span::{Attributes, Id},
    Event, Level, Subscriber,
};
use tracing_subscriber::{
    layer::{Context, Layer},
    registry::LookupSpan,
};

#[derive(Debug)]
pub struct HierarchicalLayer {
    stdout: io::Stdout,
    indent_amount: usize,
    ansi: bool,
    lck: Mutex<()>,
}

struct Data {
    start: DateTime<Local>,
    kvs: Vec<(&'static str, String)>,
}

struct FmtEvent<'a> {
    stdout: io::StdoutLock<'a>,
    comma: bool,
    buf: String,
}

impl<'a> FmtEvent<'a> {
    fn print(&mut self, indent: usize, indent_amount: usize) {
        let mut idt = String::with_capacity(indent * indent_amount);
        let mut i = 0;
        while i < (indent * indent_amount) {
            if i % indent_amount == 0 {
                idt.push('┃');
            } else {
                idt.push(' ');
            }
            i += 1;
        }
        let wrapper = textwrap::Wrapper::new(200 - idt.len())
            .subsequent_indent(&idt)
            .break_words(true);
        let wrapped = wrapper.wrap(&self.buf);
        for w in &wrapped[0..wrapped.len() - 1] {
            writeln!(self.stdout, "{}", w).unwrap();
        }
        write!(self.stdout, "{}", wrapped[wrapped.len() - 1]).unwrap();
    }
}

impl Data {
    fn new(attrs: &tracing::span::Attributes<'_>) -> Self {
        let mut span = Self {
            start: Local::now(),
            kvs: Vec::new(),
        };
        attrs.record(&mut span);
        span
    }
}

impl Visit for Data {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.kvs.push((field.name(), format!("{:?}", value)))
    }
}

impl<'a> Visit for FmtEvent<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        use fmt::Write;
        write!(
            &mut self.buf,
            "{comma} ",
            comma = if self.comma { "," } else { "" },
        )
        .unwrap();
        let name = field.name();
        if name == "message" {
            write!(&mut self.buf, "{:?}", value).unwrap();
            self.comma = true;
        } else {
            write!(&mut self.buf, "{}={:?}", name, value).unwrap();
            self.comma = true;
        }
    }
}

struct ColorLevel<'a>(&'a Level);

impl<'a> fmt::Display for ColorLevel<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self.0 {
            Level::TRACE => Color::Purple.bold().paint("TRACE"),
            Level::DEBUG => Color::Blue.bold().paint("DEBUG"),
            Level::INFO => Color::Green.bold().paint(" INFO"),
            Level::WARN => Color::RGB(252, 234, 160).bold().paint(" WARN"), // orange
            Level::ERROR => Color::Red.bold().paint("ERROR"),
        }
        .fmt(f)
    }
}

impl HierarchicalLayer {
    pub fn new(indent_amount: usize) -> Self {
        let ansi = atty::is(atty::Stream::Stdout);
        Self {
            indent_amount,
            stdout: io::stdout(),
            ansi,
            lck: Mutex::new(()),
        }
    }

    pub fn with_ansi(self, ansi: bool) -> Self {
        Self { ansi, ..self }
    }

    fn styled(&self, style: Style, text: impl AsRef<str>) -> String {
        if self.ansi {
            style.paint(text.as_ref()).to_string()
        } else {
            text.as_ref().to_string()
        }
    }

    fn append_kvs<'a, I, K, V>(
        &self,
        // writer: &mut impl io::Write,
        buf: &mut impl fmt::Write,
        kvs: I,
        leading: &str,
    ) -> fmt::Result
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str> + 'a,
        V: fmt::Display + 'a,
    {
        let mut kvs = kvs.into_iter();
        if let Some((k, v)) = kvs.next() {
            write!(
                buf,
                "{}{}={}",
                leading,
                // Style::new().fg(Color::Purple).bold().paint(k.as_ref()),
                k.as_ref(),
                v
            )?;
        }
        for (k, v) in kvs {
            write!(
                buf,
                ", {}={}",
                // Style::new().fg(Color::Purple).bold().paint(k.as_ref()),
                k.as_ref(),
                v
            )?;
        }
        Ok(())
    }

    fn print(
        &self,
        writer: &mut impl io::Write,
        buf: &str,
        indent: usize,
        name_len: usize,
    ) -> io::Result<()> {
        let mut idt = String::with_capacity(indent * self.indent_amount);
        let mut i = 0;
        while i < (indent * self.indent_amount) {
            if i % self.indent_amount == 0 {
                idt.push('┃');
            } else {
                idt.push(' ');
            }
            i += 1;
        }
        let wrapper = textwrap::Wrapper::new(200 + name_len - idt.len())
            .initial_indent(&idt)
            .subsequent_indent(&idt)
            .break_words(true);
        for w in wrapper.wrap_iter(&buf) {
            writeln!(writer, "{}", w)?;
        }
        Ok(())
    }

    fn print_indent(&self, writer: &mut impl io::Write, indent: usize) -> io::Result<()> {
        const LINE: &str = "┣━";
        let mut i = 0;
        while i < ((indent - 1) * self.indent_amount) {
            if i % self.indent_amount == 0 {
                write!(writer, "┃")?;
            } else {
                write!(writer, " ")?;
            }
            i += 1;
        }
        write!(writer, "{}", LINE)?;
        for _ in 0..self.indent_amount.saturating_sub(2) / 2 {
            write!(writer, "━")?;
        }
        Ok(())
    }
}

impl<S> Layer<S> for HierarchicalLayer
where
    S: Subscriber + for<'span> LookupSpan<'span> + fmt::Debug,
{
    fn new_span(&self, attrs: &Attributes, id: &Id, ctx: Context<S>) {
        let data = Data::new(attrs);
        let span = ctx.span(id).expect("in new_span but span does not exist");
        span.extensions_mut().insert(data);
    }

    fn on_enter(&self, id: &tracing::Id, ctx: Context<S>) {
        let mut stdout = self.stdout.lock();
        let span = ctx.span(&id).expect("in on_enter but span does not exist");
        let ext = span.extensions();
        let data = ext.get::<Data>().expect("span does not have data");

        let indent = ctx.scope().collect::<Vec<_>>().len() - 1;
        // self.print_indent(&mut stdout, indent)
        //     .expect("Unable to write to stdout");

        let mut buf = String::new();

        use fmt::Write;

        let name = span.metadata().name();

        write!(
            &mut buf,
            "{name}",
            name = self.styled(Style::new().fg(Color::Green).bold(), name)
        )
        .unwrap();
        write!(
            &mut buf,
            "{}",
            self.styled(Style::new().fg(Color::Green).bold(), "{") // Style::new().fg(Color::Green).dimmed().paint("{")
        )
        .unwrap();
        self.append_kvs(&mut buf, data.kvs.iter().map(|(k, v)| (k, v)), "")
            .unwrap();
        write!(
            &mut buf,
            "{}",
            self.styled(Style::new().fg(Color::Green).bold(), "}") // Style::new().dimmed().paint("}")
        )
        .unwrap();
        let _guard = self.lck.lock().unwrap();
        self.print(&mut stdout, &buf, indent, name.len()).unwrap();
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<S>) {
        let mut stdout = self.stdout.lock();
        // printing the indentation
        let indent = if let Some(_) = ctx.current_span().id() {
            // size hint isn't implemented on Scope.
            let indent = ctx.scope().collect::<Vec<_>>().len();
            self.print_indent(&mut stdout, indent)
                .expect("Unable to write to stdout");
            indent
        } else {
            0
        };

        // check if this event occurred in the context of a span.
        // if it has, get the start time of this span.
        let start = match ctx.current_span().id() {
            Some(id) => match ctx.span(id) {
                // if the event is in a span, get the span's starting point.
                Some(ctx) => {
                    let ext = ctx.extensions();
                    let data = ext
                        .get::<Data>()
                        .expect("Data cannot be found in extensions");
                    Some(data.start)
                }
                None => None,
            },
            None => None,
        };
        let now = Local::now();
        if let Some(start) = start {
            let elapsed = now - start;
            let level = event.metadata().level();
            let level = if self.ansi {
                ColorLevel(level).to_string()
            } else {
                level.to_string()
            };
            write!(
                &mut stdout,
                "{timestamp}{unit} {level}",
                timestamp = self.styled(
                    Style::new().dimmed(),
                    elapsed.num_milliseconds().to_string()
                ),
                unit = self.styled(Style::new().dimmed(), "ms"),
                level = level,
            )
            .expect("Unable to write to stdout");
        }
        let mut visitor = FmtEvent {
            stdout,
            comma: false,
            buf: String::new(),
        };
        event.record(&mut visitor);
        let _guard = self.lck.lock();
        visitor.print(indent, self.indent_amount);
        writeln!(&mut visitor.stdout).unwrap();
    }

    fn on_close(&self, _: Id, _: Context<S>) {}
}
