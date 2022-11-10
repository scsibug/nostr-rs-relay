pub enum EventResultStatus {
    Saved,
    Duplicate,
    Invalid,
    Blocked,
    RateLimited,
    Error,
}

pub struct EventResult {
    pub id: String,
    pub msg: String,
    pub status: EventResultStatus,
}

pub enum Notice {
    Message(String),
    EventResult(EventResult),
}

impl EventResultStatus {
    pub fn to_bool(&self) -> bool {
        match self {
            Self::Saved => true,
            Self::Duplicate => true,
            Self::Invalid => false,
            Self::Blocked => false,
            Self::RateLimited => false,
            Self::Error => false,
        }
    }

    pub fn prefix(&self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Duplicate => "duplicate",
            Self::Invalid => "invalid",
            Self::Blocked => "blocked",
            Self::RateLimited => "rate-limited",
            Self::Error => "error",
        }
    }
}

impl Notice {
    //pub fn err(err: error::Error, id: String) -> Notice {
    //    Notice::err_msg(format!("{}", err), id)
    //}

    pub fn message(msg: String) -> Notice {
        Notice::Message(msg)
    }

    fn prefixed(id: String, msg: &str, status: EventResultStatus) -> Notice {
        let msg = format!("{}: {}", status.prefix(), msg);
        Notice::EventResult(EventResult { id, msg, status })
    }

    pub fn invalid(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Invalid)
    }

    pub fn blocked(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Blocked)
    }

    pub fn rate_limited(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::RateLimited)
    }

    pub fn duplicate(id: String) -> Notice {
        Notice::prefixed(id, "", EventResultStatus::Duplicate)
    }

    pub fn error(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Error)
    }

    pub fn saved(id: String) -> Notice {
        Notice::EventResult(EventResult {
            id,
            msg: "".into(),
            status: EventResultStatus::Saved,
        })
    }
}
