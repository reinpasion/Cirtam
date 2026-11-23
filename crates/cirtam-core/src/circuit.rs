use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point(pub i32, pub i32);

impl Point {
    pub fn offset(self, delta: Self) -> Self {
        Self(self.0 + delta.0, self.1 + delta.1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Resistor,
    Voltage,
    Current,
    Ground,
    Voltmeter,
    Ammeter,
}

impl Kind {
    pub const ALL: [Self; 6] = [
        Self::Resistor,
        Self::Voltage,
        Self::Current,
        Self::Ground,
        Self::Voltmeter,
        Self::Ammeter,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Resistor => "Resistor",
            Self::Voltage => "DC voltage source",
            Self::Current => "DC current source",
            Self::Ground => "Ground",
            Self::Voltmeter => "Voltmeter",
            Self::Ammeter => "Ammeter",
        }
    }

    pub fn unit(self) -> Option<&'static str> {
        match self {
            Self::Resistor => Some("Ω"),
            Self::Voltage => Some("V"),
            Self::Current => Some("A"),
            _ => None,
        }
    }

    pub fn default_value(self) -> f64 {
        match self {
            Self::Resistor => 1000.0,
            Self::Voltage => 5.0,
            Self::Current => 0.001,
            _ => 0.0,
        }
    }

    pub fn check_value(self, value: f64) -> Result<(), String> {
        if !value.is_finite() {
            return Err("Enter a finite value.".into());
        }
        if self == Self::Resistor && value <= 0.0 {
            return Err("Resistance must be greater than zero.".into());
        }
        if matches!(self, Self::Ground | Self::Ammeter | Self::Voltmeter) && value != 0.0 {
            return Err(
                "The stored value must be zero. Open Details and click Apply to repair it.".into(),
            );
        }
        Ok(())
    }

    pub fn parse_value(self, input: &str) -> Result<f64, String> {
        let input = input.trim();
        let (number, multiplier) = match input.chars().last() {
            Some('p') => (&input[..input.len() - 1], 1e-12),
            Some('n') => (&input[..input.len() - 1], 1e-9),
            Some('u' | 'µ' | 'μ') => (&input[..input.char_indices().last().unwrap().0], 1e-6),
            Some('m') => (&input[..input.len() - 1], 1e-3),
            Some('k') => (&input[..input.len() - 1], 1e3),
            Some('M') => (&input[..input.len() - 1], 1e6),
            Some('G') => (&input[..input.len() - 1], 1e9),
            _ => (input, 1.0),
        };
        let value = number
            .trim_end()
            .parse::<f64>()
            .map_err(|_| "Enter a number such as 4.7k or 2m.".to_owned())?
            * multiplier;
        self.check_value(value)?;
        Ok(value)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Component {
    #[serde(default, skip_serializing_if = "Ratings::is_empty")]
    pub ratings: Ratings,
    pub kind: Kind,
    pub at: Point,
    pub rotation: u8,
    pub label: String,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_ohms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ratings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_watts: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance_percent: Option<f64>,
}

impl Ratings {
    fn is_empty(&self) -> bool {
        self.power_watts.is_none() && self.tolerance_percent.is_none()
    }

    pub fn validate(self, kind: Kind) -> Result<(), String> {
        if kind != Kind::Resistor && !self.is_empty() {
            return Err("These ratings apply only to resistors.".into());
        }
        if self
            .power_watts
            .is_some_and(|watts| !watts.is_finite() || watts <= 0.0)
        {
            return Err("Power rating must be a finite number greater than zero.".into());
        }
        if self
            .tolerance_percent
            .is_some_and(|percent| !(0.0..=100.0).contains(&percent))
        {
            return Err("Tolerance must be between 0 and 100 percent.".into());
        }
        Ok(())
    }
}

impl Component {
    pub fn validate_values(&self) -> Result<(), String> {
        self.kind.check_value(self.value)?;
        self.ratings.validate(self.kind)?;
        if let Some(actual) = self.actual_ohms {
            let tolerance = self
                .ratings
                .tolerance_percent
                .filter(|_| self.kind == Kind::Resistor)
                .ok_or("A sampled resistance requires resistor tolerance.")?
                / 100.0;
            if !actual.is_finite()
                || actual <= 0.0
                || actual < self.value * (1.0 - tolerance)
                || actual > self.value * (1.0 + tolerance)
            {
                return Err(
                    "The stored actual resistance must be positive and within tolerance.".into(),
                );
            }
        }
        Ok(())
    }

    pub fn effective_resistance(&self) -> f64 {
        self.actual_ohms.unwrap_or(self.value)
    }

    pub fn new(kind: Kind, at: Point) -> Self {
        Self {
            kind,
            ratings: Ratings::default(),
            at,
            rotation: 0,
            label: String::new(),
            value: kind.default_value(),
            actual_ohms: None,
        }
    }

    pub fn terminal(&self, index: usize) -> Point {
        let direction =
            [Point(1, 0), Point(0, 1), Point(-1, 0), Point(0, -1)][usize::from(self.rotation % 4)];
        let distance = if self.kind == Kind::Ground || index == 0 {
            -2
        } else {
            2
        };
        self.at
            .offset(Point(direction.0 * distance, direction.1 * distance))
    }

    pub fn terminals(&self) -> impl Iterator<Item = Point> + '_ {
        (0..if self.kind == Kind::Ground { 1 } else { 2 }).map(|i| self.terminal(i))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wire(pub Point, pub Point);

impl Wire {
    pub fn contains(self, p: Point) -> bool {
        let Self(a, b) = self;
        let (x, y) = (
            i64::from(b.0) - i64::from(a.0),
            i64::from(b.1) - i64::from(a.1),
        );
        let (px, py) = (
            i64::from(p.0) - i64::from(a.0),
            i64::from(p.1) - i64::from(a.1),
        );
        (a.0.min(b.0)..=a.0.max(b.0)).contains(&p.0)
            && (a.1.min(b.1)..=a.1.max(b.1)).contains(&p.1)
            && x * py == y * px
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Circuit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connections: Option<crate::topology::Connections>,
    pub components: Vec<Component>,
    pub wires: Vec<Wire>,
}

impl Circuit {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_geometry()?;
        for component in &self.components {
            component
                .validate_values()
                .map_err(|error| format!("{}: {error}", component.kind.name()))?;
        }
        Ok(())
    }

    pub fn validate_geometry(&self) -> Result<(), String> {
        let valid_point =
            |p: Point| (-100_000..=100_000).contains(&p.0) && (-100_000..=100_000).contains(&p.1);
        for c in &self.components {
            if !valid_point(c.at) || c.rotation > 3 {
                return Err("Invalid component position or rotation.".into());
            }
        }
        for &Wire(a, b) in &self.wires {
            if !valid_point(a) || !valid_point(b) || a == b {
                return Err("Wires must have distinct endpoints within the grid.".into());
            }
        }
        if let Some(connections) = &self.connections {
            connections.validate(self)?;
        }
        Ok(())
    }
}
