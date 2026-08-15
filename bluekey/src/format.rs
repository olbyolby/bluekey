use std::{fmt::Display, io::Write, ops::Deref};

// A grouping of values printed with a seperateor between them 
pub struct Group<S, T> {
    seperator: T,
    stream: S,
    has_single: bool,
}
impl<S: Write, T: AsRef<[u8]>> Group<S, T> {
    pub fn new(stream: S, seperator: T) -> Self {
        Self {
            seperator: seperator,
            stream,
            has_single: false,
        }
    }
    pub fn next(&mut self) -> Result<&mut S, std::io::Error> {
        if self.has_single {
            self.stream.write(self.seperator.as_ref())?;
        }

        self.has_single = true;
        Ok(&mut self.stream)
    }
}

pub trait Groupable: Sized + Write {
    fn group<T: AsRef<[u8]>>(&mut self, seperator: T) -> Group<&mut Self, T> {
        Group::new(self, seperator)
    }

    fn into_group<T: AsRef<[u8]>>(self, seperator: T) -> Group<Self, T> {
        Group::new(self, seperator)
    }
}
impl<T: Write> Groupable for T {}


// Both the ANSI code to apply a certain formatting and to remove that formatting
#[derive(Clone, Copy)]
pub struct AnsiFormat<'a> {
    enter: &'a str,
    exit: &'a str
}
impl<'a> AnsiFormat<'a> {
    pub const fn new(enter: &'a str, exit: &'a str) -> Self {
        Self { enter, exit }
    }
    pub fn wrap<'b, T: Display> (self, what: &'b T) -> AnsiWrap<'b, T> where 'a: 'b {
        AnsiWrap { 
            format: self, 
            body: what 
        }
    }
}

// Wrap a value to use a particular ANSI wrapping when displayed
pub struct AnsiWrap<'a, T> {
    format: AnsiFormat<'a>,
    body: &'a T
}
impl<'a, T: Display> Display for AnsiWrap<'a, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}{}", self.format.enter, self.body, self.format.exit)
    }
}
impl<'a, T> Deref for AnsiWrap<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.body
    }
}

