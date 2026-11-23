use cirtam_core::circuit::{Component, Kind, Ratings};

pub fn edit_values(
    component: &mut Component,
    nominal: f64,
    ratings: Ratings,
) -> Result<(), String> {
    component.kind.check_value(nominal)?;
    if component.kind != Kind::Resistor {
        component.value = nominal;
        component.ratings = Ratings::default();
        component.actual_ohms = None;
        return Ok(());
    }

    ratings.validate(component.kind)?;
    let needs_sample = component.value != nominal
        || component.ratings.tolerance_percent != ratings.tolerance_percent;
    if needs_sample {
        component.actual_ohms = sample(component.kind, nominal, ratings.tolerance_percent)?;
    }
    component.value = nominal;
    component.ratings = ratings;
    Ok(())
}

pub fn resample(component: &mut Component) -> Result<(), String> {
    component.ratings.validate(component.kind)?;
    component.actual_ohms = sample(
        component.kind,
        component.value,
        component.ratings.tolerance_percent,
    )?;
    Ok(())
}

fn sample(kind: Kind, nominal: f64, tolerance: Option<f64>) -> Result<Option<f64>, String> {
    let Some(tolerance) = tolerance.filter(|_| kind == Kind::Resistor) else {
        return Ok(None);
    };
    kind.check_value(nominal)?;
    if tolerance == 0.0 {
        return Ok(Some(nominal));
    }

    let fraction = tolerance / 100.0;
    let upper = nominal * (1.0 + fraction);
    if !upper.is_finite() {
        return Err("The resistance tolerance range exceeds the numeric range.".into());
    }
    let bits = getrandom::u64().map_err(|error| format!("Could not sample resistance. {error}"))?;
    // An open uniform interval avoids sampling zero resistance at ±100%.
    let unit = ((bits >> 12) as f64 + 0.5) / (1_u64 << 52) as f64;
    let actual = nominal * (1.0 + (2.0 * unit - 1.0) * fraction);
    Ok(Some(actual.max(f64::from_bits(1)).min(upper)))
}
