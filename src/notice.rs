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
    #[must_use] pub fn to_bool(&self) -> bool {
        match self {
            Self::Duplicate | Self::Saved => true,
            Self::Invalid |Self::Blocked | Self::RateLimited | Self::Error => false,
        }
    }

    #[must_use] pub fn prefix(&self) -> &'static str {
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

    #[must_use] pub fn message(msg: String) -> Notice {
        Notice::Message(msg)
    }

    fn prefixed(id: String, msg: &str, status: EventResultStatus) -> Notice {
        let msg = format!("{}: {}", status.prefix(), msg);
        Notice::EventResult(EventResult { id, msg, status })
    }

    #[must_use] pub fn invalid(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Invalid)
    }

    #[must_use] pub fn blocked(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Blocked)
    }

    #[must_use] pub fn rate_limited(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::RateLimited)
    }

    #[must_use] pub fn duplicate(id: String) -> Notice {
        Notice::prefixed(id, "", EventResultStatus::Duplicate)
    }

    #[must_use] pub fn error(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Error)
    }

    #[must_use] pub fn saved(id: String) -> Notice {
        Notice::EventResult(EventResult {
            id,
            msg: "".into(),
            status: EventResultStatus::Saved,
        })
    }
}
