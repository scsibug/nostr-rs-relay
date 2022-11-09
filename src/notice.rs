use crate::error;

pub enum EventResultStatus {
    Saved,
    Duplicate,
    Error(String),
}

pub struct EventResult {
    pub id: String,
    pub status: EventResultStatus,
}

pub enum Notice {
    Message(String),
    EventResult(EventResult),
}

impl Notice {
    pub fn err(err: error::Error, id: String) -> Notice {
        Notice::err_msg(format!("{}", err), id)
    }

    pub fn message(msg: String) -> Notice {
        Notice::Message(msg)
    }

    pub fn saved(id: String) -> Notice {
        Notice::EventResult(EventResult {
            id,
            status: EventResultStatus::Saved,
        })
    }

    pub fn duplicate(id: String) -> Notice {
        Notice::EventResult(EventResult {
            id,
            status: EventResultStatus::Duplicate,
        })
    }

    pub fn err_msg(msg: String, id: String) -> Notice {
        Notice::EventResult(EventResult {
            id,
            status: EventResultStatus::Error(msg),
        })
    }
}
