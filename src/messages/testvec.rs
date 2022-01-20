#[allow(dead_code)]
pub mod event {
    pub const VALID_EVENT: &str = r#"
{
    "id": "5436cab31e64e4f2cbd6216a68d95369210174fa4a82e77d09184aa51806de60",
    "pubkey": "5ac9d737d7f18933967a065b007afe63e03ce9c83bd0da4c491c0810f597d2dd",
    "created_at": 1642540678,
    "kind": 2,
    "tags": [
        [
        "e",
        "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
        ],
        [
        "e",
        "bdefa1da005259928d0ad0baed9c460945b4b82618c4551de6e95ee09ece25d2"
        ],
        [
        "p",
        "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
        ],
        [
        "p",
        "859d23503e69157eb3e22bc225687d33b6ab514ae53602e99d7d75b59393e62d"
        ],
        [
        "e",
        "4d7ec2e01be6ba5111a0a0ce821639885ea4cb8f4a652accd911dfb7d5151e17"
        ],
        [
        "p",
        "cfc5794db955d560b8aec1bd0f27d41d52e1d9d0b157057f7501ea3d30dca034"
        ]
    ],
    "content": "Some String Content",
    "sig": "e947a5c4a65eefd08292e8a8d995fb8b9e43a0f2ddeda57086ddecd0a9f84c2c2b59ae28e92838412268d1e4d091d4d4319661403a4641e18457e24fc7bda0f8"
    }
"#;

    pub const ID_INVALID: &str = r#"
{
    "id": "5436cab34e4f2cbd216a68d95369210174fa4a82e77d09184aa51806de60",
    "pubkey": "5ac9d737d7f18933967a065b007afe63e03ce9c83bd0da4c491c0810f597d2dd",
    "created_at": 1642540678,
    "kind": 2,
    "tags": [
        [
        "e",
        "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
        ],
        [
        "p",
        "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
        ]
    ],
    "content": "Some String Content",
    "sig": "e947a5c4a65eefd08292e8a8d995fb8b9e43a0f2ddeda57086ddecd0a9f84c2c2b59ae28e92838412268d1e4d091d4d4319661403a4641e18457e24fc7bda0f8"
    }
"#;

    pub const PUBKEY_MALFORMED: &str = r#"
{
    "id": "5436cab31e64e4f2cbd6216a68d95369210174fa4a82e77d09184aa51806de60",
    "pubkey": "5ac9d737d7f18933967b007afe63e03ce9c83bd0da4c491c0810f597d2dd",
    "created_at": 1642540678,
    "kind": 2,
    "tags": [
        [
        "e",
        "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
        ],
        [
        "p",
        "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
        ]
    ],
    "content": "Some String Content",
    "sig": "e947a5c4a65eefd08292e8a8d995fb8b9e43a0f2ddeda57086ddecd0a9f84c2c2b59ae28e92838412268d1e4d091d4d4319661403a4641e18457e24fc7bda0f8"
    }
"#;

    pub const SIG_MALFORMED: &str = r#"
{
    "id": "5436cab31e64e4f2cbd6216a68d95369210174fa4a82e77d09184aa51806de60",
    "pubkey": "5ac9d737d7f18933967a065b007afe63e03ce9c83bd0da4c491c0810f597d2dd",
    "created_at": 1642540678,
    "kind": 2,
    "tags": [
        [
        "e",
        "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
        ],
        [
        "p",
        "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
        ]
    ],
    "content": "Some String Content",
    "sig": "e947a5c4a65eefd08292e8afb8b9e43a0f2ddeda57086ddecd0a9f84c2c2b59ae28e92838412268d1e4d091d4d4319661403a4641e18457e24fc7bda0f8"
    }
"#;

    pub const WRONG_EVENT_ID: &str = r#"
{
    "id": "5436cab31e64e4f2cbd6216a68d95369210174fa4a82e77d09184aa51806de60",
    "pubkey": "5ac9d737d7f18933967a065b007afe63e03ce9c83bd0da4c491c0810f597d2dd",
    "created_at": 1642540678,
    "kind": 2,
    "tags": [
        [
        "e",
        "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
        ],
        [
        "p",
        "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
        ]
    ],
    "content": "Some String Content",
    "sig": "14d0bf1a8953506fb460f58be141af767fd112535fb3922ef217308e2c26706f1eeb432b3dba9a01082f9e4d4ef5678ad0d9d532c0dfa907b568722d0b0119ba"
    }
"#;

    pub const MISSING_FIELD: &str = r#"
{
    "id": "5436cab31e64e4f2cbd6216a68d95369210174fa4a82e77d09184aa51806de60",
    "pubkey": "5ac9d737d7f18933967a065b007afe63e03ce9c83bd0da4c491c0810f597d2dd",
    "kind": 2,
    "tags": [
        [
        "e",
        "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
        ],
        [
        "p",
        "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
        ]
    ],
    "content": "Some String Content",
    "sig": "e947a5c4a65eefd08292e8a8d995fb8b9e43a0f2ddeda57086ddecd0a9f84c2c2b59ae28e92838412268d1e4d091d4d4319661403a4641e18457e24fc7bda0f8"
    }
"#;
}

#[allow(dead_code)]
pub mod subscription {
    pub const SUBS: &str = r##"
    [
        "REQ",
        "subscription_id",
        {
            "ids": [
                "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
            ],
            "kinds": [
                1
            ],
            "#e": [
                "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
            ],
            "#p": [
                "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
            ],
            "since": 1642677735,
            "until": 1642677735,
            "authors": [
                "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
            ]
        }
    ]
    "##;

    pub const ID_SUBS: &str = r##"
    [
        "REQ",
        "subscription_id",
        {
            "ids": [
                "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa",
                "5436cab31e64e4f2cbd6216a68d95369210174fa4a82e77d09184aa51806de60",
                "4d7ec2e01be6ba5111a0a0ce821639885ea4cb8f4a652accd911dfb7d5151e17"
            ]
        }
    ]
    "##;

    pub const AUTHORS_SUBS: &str = r##"
    [
        "REQ",
        "subscription_id",
        {
            "authors": [
                "5ac9d737d7f18933967a065b007afe63e03ce9c83bd0da4c491c0810f597d2dd",
                "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9",
                "859d23503e69157eb3e22bc225687d33b6ab514ae53602e99d7d75b59393e62d"
            ]
        }
    ]
    "##;

    pub const KINDS_SUBS: &str = r##"
    [
        "REQ",
        "subscription_id",
        {
            "kinds": [
                0,
                1,
                2
            ]
        }
    ]
    "##;

    pub const SINCE_SUBS: &str = r##"
    [
        "REQ",
        "subscription_id",
        {
            "since": 1642540500
        }
    ]
    "##;

    pub const UNTIL_SUBS: &str = r##"
    [
        "REQ",
        "subscription_id",
        {
            "until": 1642540800
        }
    ]
    "##;

    pub const EVENT_TAGS_SUBS: &str = r##"
    [
        "REQ",
        "subscription_id",
        {
            "#e": [
                "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa",
                "e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443",
                "859d23503e69157eb3e22bc225687d33b6ab514ae53602e99d7d75b59393e62d"
            ]
        }
    ]
    "##;

    pub const PUBKEY_TAGS_SUBS: &str = r##"
    [
        "REQ",
        "subscription_id",
        {
            "#p": [
                "859d23503e69157eb3e22bc225687d33b6ab514ae53602e99d7d75b59393e62d",
                "41cc121c419921942add6db6482fb36243faf83317c866d2a28d8c6d7089f7ba",
                "e6642fd69bd211f93f7f1f36ca51a26a5290eb2dd1b0d8279a87bb0d480c8443"
            ]
        }
    ]
    "##;
}
