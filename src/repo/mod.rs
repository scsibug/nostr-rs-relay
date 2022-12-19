use crate::error::{Result};
use crate::event::{Event};

pub mod sqlite;

pub trait Repo {
    fn write_event(self: &mut Self, e: &Event) -> Result<usize>;
}
