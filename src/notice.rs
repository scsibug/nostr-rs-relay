pub enum EventResultStatus {
    Saved,
    Duplicate,
    Invalid,
    Blocked,
    RateLimited,
    Error,
    Restricted,
}

pub struct EventResult {
    pub id: String,
    pub msg: String,
    pub status: EventResultStatus,
}

pub enum Notice {
    Message(String),
    EventResult(EventResult),
    AuthChallenge(String),
}

impl EventResultStatus {
    #[must_use]
    pub fn to_bool(&self) -> bool {
        match self {
            Self::Duplicate | Self::Saved => true,
            Self::Invalid | Self::Blocked | Self::RateLimited | Self::Error | Self::Restricted => {
                false
            }
        }
    }

    #[must_use]
    pub fn prefix(&self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Duplicate => "duplicate",
            Self::Invalid => "invalid",
            Self::Blocked => "blocked",
            Self::RateLimited => "rate-limited",
            Self::Error => "error",
            Self::Restricted => "restricted",
        }
    }
}

impl Notice {
    //pub fn err(err: error::Error, id: String) -> Notice {
    //    Notice::err_msg(format!("{}", err), id)
    //}

    #[must_use]
    pub fn message(msg: String) -> Notice {
        Notice::Message(msg)
    }

    fn prefixed(id: String, msg: &str, status: EventResultStatus) -> Notice {
        let msg = if has_machine_readable_prefix(msg) {
            msg.to_owned()
        } else {
            format!("{}: {}", status.prefix(), msg)
        };
        Notice::EventResult(EventResult { id, msg, status })
    }

    #[must_use]
    pub fn invalid(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Invalid)
    }

    #[must_use]
    pub fn blocked(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Blocked)
    }

    #[must_use]
    pub fn rate_limited(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::RateLimited)
    }

    #[must_use]
    pub fn duplicate(id: String) -> Notice {
        Notice::prefixed(id, "", EventResultStatus::Duplicate)
    }

    #[must_use]
    pub fn error(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Error)
    }

    #[must_use]
    pub fn restricted(id: String, msg: &str) -> Notice {
        Notice::prefixed(id, msg, EventResultStatus::Restricted)
    }

    #[must_use]
    pub fn saved(id: String) -> Notice {
        Notice::EventResult(EventResult {
            id,
            msg: "".into(),
            status: EventResultStatus::Saved,
        })
    }
}

fn has_machine_readable_prefix(msg: &str) -> bool {
    [
        "auth-required:",
        "blocked:",
        "duplicate:",
        "error:",
        "invalid:",
        "pow:",
        "rate-limited:",
        "restricted:",
    ]
    .iter()
    .any(|prefix| msg.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::{EventResultStatus, Notice};

    #[test]
    fn blocked_preserves_auth_required_prefix() {
        let notice = Notice::blocked("event-id".to_owned(), "auth-required: sign in first");

        match notice {
            Notice::EventResult(result) => {
                assert_eq!(result.id, "event-id");
                assert_eq!(result.msg, "auth-required: sign in first");
                assert!(matches!(result.status, EventResultStatus::Blocked));
            }
            _ => panic!("expected event result"),
        }
    }

    #[test]
    fn blocked_prefixes_plain_messages() {
        let notice = Notice::blocked("event-id".to_owned(), "policy denied");

        match notice {
            Notice::EventResult(result) => {
                assert_eq!(result.id, "event-id");
                assert_eq!(result.msg, "blocked: policy denied");
                assert!(matches!(result.status, EventResultStatus::Blocked));
            }
            _ => panic!("expected event result"),
        }
    }
}
