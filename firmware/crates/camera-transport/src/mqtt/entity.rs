//! The capability catalog a camera hands to the MQTT layer: one [`Entity`] per Home Assistant entity.
//! The camera lists only what its hardware really offers; the layer derives topics, discovery and
//! command validation from this one list.

/// How Home Assistant draws a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Slider,
    Box,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Domain {
    Number {
        min: i32,
        max: i32,
        step: i32,
        mode: Mode,
    },
    Switch,
    /// A select without options is not offered: its discovery config is cleared.
    Select {
        options: Vec<String>,
    },
    Button,
    Sensor,
    BinarySensor,
    Cover,
    Event {
        types: Vec<&'static str>,
    },
    Text,
}

impl Domain {
    /// A number with step 1.
    pub fn number(min: i32, max: i32, mode: Mode) -> Self {
        Self::Number {
            min,
            max,
            step: 1,
            mode,
        }
    }
    pub fn select<S: Into<String>>(options: impl IntoIterator<Item = S>) -> Self {
        Self::Select {
            options: options.into_iter().map(Into::into).collect(),
        }
    }
    /// The Home Assistant component name, which is also the discovery topic segment.
    pub fn component(&self) -> &'static str {
        match self {
            Self::Number { .. } => "number",
            Self::Switch => "switch",
            Self::Select { .. } => "select",
            Self::Button => "button",
            Self::Sensor => "sensor",
            Self::BinarySensor => "binary_sensor",
            Self::Cover => "cover",
            Self::Event { .. } => "event",
            Self::Text => "text",
        }
    }
    /// Home Assistant sends commands to this entity.
    pub fn commands(&self) -> bool {
        matches!(
            self,
            Self::Number { .. }
                | Self::Switch
                | Self::Select { .. }
                | Self::Button
                | Self::Cover
                | Self::Text
        )
    }
    /// The entity has a state (or event) topic.
    pub fn has_state(&self) -> bool {
        !matches!(self, Self::Button)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Config,
    Diagnostic,
}

/// How a command for the entity is dispatched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Control {
    /// The camera's parser receives the entity id.
    #[default]
    None,
    /// The layer validates the payload against the domain and yields `Command::Setting { slot, value }`.
    Slot(u16),
    /// The camera's parser receives this route name instead of the entity id (several entities may
    /// share one command topic, such as a gate's cover and its buttons).
    Route(&'static str),
}

/// Topic paths under the camera prefix that replace the defaults `<id>/state` and `<id>/set`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Topics {
    pub state: Option<String>,
    pub command: Option<String>,
}

/// Payloads that replace the defaults (`ON`/`OFF`, `PRESS`, `OPEN`/`CLOSE`, `open`/`closed`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Payloads {
    pub on: Option<&'static str>,
    pub off: Option<&'static str>,
    pub press: Option<&'static str>,
    pub open: Option<&'static str>,
    pub close: Option<&'static str>,
    pub state_open: Option<&'static str>,
    pub state_closed: Option<&'static str>,
    pub value_template: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entity {
    /// Identity under the node: unique id `<node>_<id>`, default topics `<prefix>/<id>/...`.
    pub id: String,
    /// `None` publishes `"name": null` (the entity takes the device's name).
    pub name: Option<String>,
    pub domain: Domain,
    pub category: Option<Category>,
    pub device_class: Option<&'static str>,
    pub icon: Option<&'static str>,
    pub unit: Option<&'static str>,
    pub state_class: Option<&'static str>,
    pub enabled_by_default: bool,
    /// State id whose topic carries this entity's JSON attributes: the state topic of the entity
    /// with that id, or `<prefix>/<id>/state` when no entity has it.
    pub attributes: Option<String>,
    pub topics: Option<Topics>,
    pub payloads: Option<Payloads>,
    pub control: Control,
}

impl Entity {
    pub fn new(id: impl Into<String>, name: impl Into<String>, domain: Domain) -> Self {
        Self {
            id: id.into(),
            name: Some(name.into()),
            domain,
            category: None,
            device_class: None,
            icon: None,
            unit: None,
            state_class: None,
            enabled_by_default: true,
            attributes: None,
            topics: None,
            payloads: None,
            control: Control::None,
        }
    }
    /// Publishes `"name": null`.
    pub fn unnamed(mut self) -> Self {
        self.name = None;
        self
    }
    pub fn category(mut self, category: Category) -> Self {
        self.category = Some(category);
        self
    }
    pub fn device_class(mut self, class: &'static str) -> Self {
        self.device_class = Some(class);
        self
    }
    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn unit(mut self, unit: &'static str) -> Self {
        self.unit = Some(unit);
        self
    }
    pub fn state_class(mut self, class: &'static str) -> Self {
        self.state_class = Some(class);
        self
    }
    pub fn disabled_by_default(mut self) -> Self {
        self.enabled_by_default = false;
        self
    }
    pub fn attributes(mut self, state_id: impl Into<String>) -> Self {
        self.attributes = Some(state_id.into());
        self
    }
    pub fn state_topic(mut self, path: impl Into<String>) -> Self {
        self.topics.get_or_insert_with(Topics::default).state = Some(path.into());
        self
    }
    pub fn command_topic(mut self, path: impl Into<String>) -> Self {
        self.topics.get_or_insert_with(Topics::default).command = Some(path.into());
        self
    }
    pub fn payloads(mut self, payloads: Payloads) -> Self {
        self.payloads = Some(payloads);
        self
    }
    pub fn slot(mut self, slot: u16) -> Self {
        self.control = Control::Slot(slot);
        self
    }
    pub fn route(mut self, route: &'static str) -> Self {
        self.control = Control::Route(route);
        self
    }

    pub(crate) fn payload(
        &self,
        pick: fn(&Payloads) -> Option<&'static str>,
        default: &'static str,
    ) -> &'static str {
        self.payloads.as_ref().and_then(pick).unwrap_or(default)
    }
    /// Topic path under the prefix, or `None` for a button.
    pub(crate) fn state_path(&self) -> Option<String> {
        self.domain.has_state().then(|| {
            self.topics
                .as_ref()
                .and_then(|t| t.state.clone())
                .unwrap_or_else(|| format!("{}/state", self.id))
        })
    }
    pub(crate) fn command_path(&self) -> Option<String> {
        self.domain.commands().then(|| {
            self.topics
                .as_ref()
                .and_then(|t| t.command.clone())
                .unwrap_or_else(|| format!("{}/set", self.id))
        })
    }
}
