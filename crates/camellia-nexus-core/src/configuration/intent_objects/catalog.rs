use super::*;

fn field(
    key: &str,
    label: &str,
    control: GuidedControl,
    options: &[&str],
    required: bool,
    advanced: bool,
) -> IntentObjectField {
    IntentObjectField {
        key: key.into(),
        label: label.into(),
        control,
        allowed_values: options.iter().map(|s| (*s).into()).collect(),
        required,
        secret: key == "password",
        advanced,
    }
}

pub fn intent_object_catalog(kind: ProgramKind) -> Vec<IntentObjectDescriptor> {
    if kind == ProgramKind::Generic {
        return Vec::new();
    }
    let protocols = if kind == ProgramKind::Xray {
        vec!["http", "socks"]
    } else {
        vec!["mixed", "http", "socks"]
    };
    let mut groups = vec![IntentObjectDescriptor {
        kind: IntentObjectKind::Listener,
        category: "local".into(),
        label: "Local proxy".into(),
        protocols: protocols.into_iter().map(str::to_owned).collect(),
        can_create: true,
        fields: vec![
            field(
                "protocol",
                "Proxy type",
                GuidedControl::Select,
                &[],
                true,
                false,
            ),
            field("port", "Port", GuidedControl::Number, &[], true, false),
            field(
                "access",
                "Who can connect",
                GuidedControl::Select,
                &["local", "lan"],
                true,
                false,
            ),
            field(
                "listen",
                "Listen address",
                GuidedControl::Text,
                &[],
                false,
                true,
            ),
            field(
                "username",
                "Username",
                GuidedControl::Text,
                &[],
                false,
                false,
            ),
            field(
                "password",
                "Password",
                GuidedControl::Text,
                &[],
                false,
                false,
            ),
            field("udp", "Allow UDP", GuidedControl::Toggle, &[], false, true),
        ],
    }];
    groups.push(IntentObjectDescriptor {
        kind: IntentObjectKind::DnsServer,
        category: "dns".into(),
        label: "DNS servers".into(),
        protocols: if kind == ProgramKind::Xray {
            vec!["system".into(), "udp".into(), "https".into()]
        } else {
            vec!["system".into(), "udp".into(), "tls".into(), "https".into()]
        },
        can_create: true,
        fields: vec![
            field(
                "protocol",
                "DNS connection",
                GuidedControl::Select,
                &[],
                true,
                false,
            ),
            field(
                "server",
                "Server address",
                GuidedControl::Text,
                &[],
                false,
                false,
            ),
            field(
                "serverPort",
                "Server port",
                GuidedControl::Number,
                &[],
                false,
                true,
            ),
            field(
                "bootstrap",
                "Resolve the server through",
                GuidedControl::Select,
                &[],
                false,
                true,
            ),
            field(
                "timeout",
                "Timeout (milliseconds)",
                GuidedControl::Number,
                &[],
                false,
                true,
            ),
        ],
    });
    if kind == ProgramKind::SingBox {
        groups.push(IntentObjectDescriptor {
            kind: IntentObjectKind::Tun,
            category: "tun".into(),
            label: "Virtual network adapter".into(),
            protocols: Vec::new(),
            can_create: true,
            fields: vec![
                field(
                    "address",
                    "Virtual network addresses",
                    GuidedControl::Text,
                    &[],
                    true,
                    false,
                ),
                field(
                    "autoRoute",
                    "Route traffic automatically",
                    GuidedControl::Toggle,
                    &[],
                    false,
                    false,
                ),
                field(
                    "strictRoute",
                    "Prevent traffic outside these routes",
                    GuidedControl::Toggle,
                    &[],
                    false,
                    true,
                ),
                field(
                    "dnsMode",
                    "DNS handling",
                    GuidedControl::Select,
                    &["disabled", "native", "hijack"],
                    false,
                    true,
                ),
                field(
                    "stack",
                    "Virtual adapter implementation",
                    GuidedControl::Select,
                    &["system", "gvisor", "mixed"],
                    false,
                    true,
                ),
                field(
                    "mtu",
                    "Packet size (MTU)",
                    GuidedControl::Number,
                    &[],
                    false,
                    true,
                ),
            ],
        });
    }
    groups.push(IntentObjectDescriptor {
        kind: IntentObjectKind::RouteRule,
        category: "routing".into(),
        label: "Traffic rule".into(),
        protocols: Vec::new(),
        can_create: true,
        fields: vec![
            field(
                "match",
                "Match",
                GuidedControl::Select,
                &["domain", "subdomain", "ip"],
                true,
                false,
            ),
            field(
                "value",
                "Domain or IP network",
                GuidedControl::Text,
                &[],
                true,
                false,
            ),
            field(
                "target",
                "Send through",
                GuidedControl::Select,
                &[],
                true,
                false,
            ),
        ],
    });
    for group in &mut groups {
        group
            .fields
            .retain(|field| match (group.kind, field.key.as_str()) {
                (IntentObjectKind::Listener, "udp") => kind == ProgramKind::Xray,
                (IntentObjectKind::DnsServer, "bootstrap") => kind == ProgramKind::SingBox,
                (IntentObjectKind::DnsServer, "timeout") => kind == ProgramKind::Xray,
                (IntentObjectKind::DnsServer, "serverPort") => kind != ProgramKind::Mihomo,
                _ => true,
            });
    }
    groups
}
