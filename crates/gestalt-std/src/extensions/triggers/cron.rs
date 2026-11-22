#![forbid(unsafe_code)]
//! 5-field POSIX cron and optional cloud schedule (`/schedule`) trigger handler.

use std::time::Duration;

use gestalt_runtime::{
    CancellationToken, RuntimeError, TriggerContext, TriggerEvent, TriggerHandler,
    TriggerInstanceConfig, current_epoch_ms,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Parsed 5-field POSIX cron schedule (`minute hour dom month dow`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronSchedule {
    minutes: [bool; 60],
    hours: [bool; 24],
    doms: [bool; 32],
    months: [bool; 13],
    dows: [bool; 7],
}

impl CronSchedule {
    /// Parse a standard 5-field cron expression (`"*/5 * * * *"`).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ArtifactError`] on invalid field count or syntax.
    pub fn parse(expr: &str) -> Result<Self, RuntimeError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(RuntimeError::ArtifactError(format!(
                "invalid cron expression '{expr}': expected 5 fields (min hour dom month dow)"
            )));
        }
        let mut minutes = [false; 60];
        let mut hours = [false; 24];
        let mut doms = [false; 32];
        let mut months = [false; 13];
        let mut dows = [false; 7];

        parse_field(fields[0], 0, 59, false, &mut minutes)?;
        parse_field(fields[1], 0, 23, false, &mut hours)?;
        parse_field(fields[2], 1, 31, false, &mut doms)?;
        parse_field(fields[3], 1, 12, false, &mut months)?;
        parse_field(fields[4], 0, 7, true, &mut dows)?;

        Ok(Self {
            minutes,
            hours,
            doms,
            months,
            dows,
        })
    }

    /// Return `true` if `epoch_ms` (adjusted by `tz_offset_minutes`) matches this schedule.
    #[must_use]
    pub fn matches_epoch_ms(&self, epoch_ms: u64, tz_offset_minutes: i32) -> bool {
        let total_secs = (epoch_ms / 1_000) as i64 + i64::from(tz_offset_minutes) * 60;
        let total_mins = total_secs.div_euclid(60);
        let minute = total_mins.rem_euclid(60) as usize;
        let total_hours = total_mins.div_euclid(60);
        let hour = total_hours.rem_euclid(24) as usize;
        let days_since_epoch = total_hours.div_euclid(24);
        let (_year, month, dom, dow) = civil_from_epoch_days(days_since_epoch);

        self.minutes[minute]
            && self.hours[hour]
            && self.doms[dom as usize]
            && self.months[month as usize]
            && self.dows[dow as usize]
    }

    /// Compute the earliest minute-aligned timestamp (ms) strictly after `after_ms`
    /// that matches this schedule (searching up to 366 days ahead).
    #[must_use]
    pub fn next_fire_after_ms(&self, after_ms: u64, tz_offset_minutes: i32) -> Option<u64> {
        let next_min_ms = ((after_ms / 60_000) + 1) * 60_000;
        // Search up to 366 days (527_040 minutes)
        for step in 0..527_040_u64 {
            let candidate = next_min_ms + step * 60_000;
            if self.matches_epoch_ms(candidate, tz_offset_minutes) {
                return Some(candidate);
            }
        }
        None
    }
}

fn parse_field(
    raw: &str,
    min: u8,
    max: u8,
    wrap_seven_to_zero: bool,
    out: &mut [bool],
) -> Result<(), RuntimeError> {
    for part in raw.split(',') {
        let (range_part, step) = match part.split_once('/') {
            Some((lhs, s)) => {
                let parsed_step: u8 = s.parse().map_err(|_| {
                    RuntimeError::ArtifactError(format!("invalid cron step '{s}' in '{raw}'"))
                })?;
                if parsed_step == 0 {
                    return Err(RuntimeError::ArtifactError(format!(
                        "cron step cannot be 0 in '{raw}'"
                    )));
                }
                (lhs, parsed_step)
            }
            None => (part, 1),
        };

        let (start, end) = if range_part == "*" {
            (min, max)
        } else if let Some((a, b)) = range_part.split_once('-') {
            let sa: u8 = a.parse().map_err(|_| {
                RuntimeError::ArtifactError(format!("invalid cron range start '{a}'"))
            })?;
            let eb: u8 = b.parse().map_err(|_| {
                RuntimeError::ArtifactError(format!("invalid cron range end '{b}'"))
            })?;
            if sa < min || eb > max || sa > eb {
                return Err(RuntimeError::ArtifactError(format!(
                    "cron range '{range_part}' out of bounds [{min}, {max}]"
                )));
            }
            (sa, eb)
        } else {
            let val: u8 = range_part.parse().map_err(|_| {
                RuntimeError::ArtifactError(format!("invalid cron value '{range_part}'"))
            })?;
            if val < min || val > max {
                return Err(RuntimeError::ArtifactError(format!(
                    "cron value {val} out of bounds [{min}, {max}]"
                )));
            }
            (val, val)
        };

        let mut cur = start;
        while cur <= end {
            let idx = if wrap_seven_to_zero && cur == 7 {
                0
            } else {
                cur as usize
            };
            if let Some(slot) = out.get_mut(idx) {
                *slot = true;
            }
            match cur.checked_add(step) {
                Some(next) => cur = next,
                None => break,
            }
        }
    }
    Ok(())
}

/// Howard Hinnant's civil-from-days algorithm returning `(year, month 1..=12, day 1..=31, dow 0..=6 Sun=0)`.
fn civil_from_epoch_days(days: i64) -> (i64, u8, u8, u8) {
    // 1970-01-01 was Thursday (4)
    let dow = (days + 4).rem_euclid(7) as u8;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let year = y + i64::from(m <= 2);
    (year, m, d, dow)
}

/// Cron & optional cloud schedule (`/schedule`) [`TriggerHandler`].
#[derive(Debug, Clone, Default)]
pub struct CronScheduleTriggerHandler;

impl TriggerHandler for CronScheduleTriggerHandler {
    fn kind(&self) -> &str {
        "cron_schedule"
    }

    fn description(&self) -> &str {
        "5-field POSIX cron and optional cloud schedule trigger (/schedule)"
    }

    fn validate(&self, config: &Value) -> Result<(), RuntimeError> {
        let expr = config
            .get("expression")
            .or_else(|| config.get("cron"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RuntimeError::ArtifactError(
                    "cron_schedule trigger requires 'expression' (5-field cron)".to_string(),
                )
            })?;
        let _ = CronSchedule::parse(expr)?;
        if let Some(tz) = config
            .get("timezone_offset_minutes")
            .and_then(Value::as_i64)
            && !(-720..=840).contains(&tz)
        {
            return Err(RuntimeError::ArtifactError(format!(
                "timezone_offset_minutes {tz} out of valid range [-720, 840]"
            )));
        }
        Ok(())
    }

    fn start(
        &self,
        instance: TriggerInstanceConfig,
        ctx: TriggerContext,
        event_tx: mpsc::UnboundedSender<TriggerEvent>,
        cancel: CancellationToken,
    ) -> Result<JoinHandle<()>, RuntimeError> {
        self.validate(&instance.config)?;
        let expr = instance
            .config
            .get("expression")
            .or_else(|| instance.config.get("cron"))
            .and_then(Value::as_str)
            .unwrap_or("* * * * *")
            .to_string();
        let schedule = CronSchedule::parse(&expr)?;
        let tz_offset = instance
            .config
            .get("timezone_offset_minutes")
            .and_then(Value::as_i64)
            .unwrap_or(0) as i32;
        let cloud_endpoint = instance
            .config
            .get("cloud_endpoint")
            .and_then(Value::as_str)
            .map(ToString::to_string);

        Ok(tokio::spawn(async move {
            loop {
                let now = current_epoch_ms();
                let Some(next_ms) = schedule.next_fire_after_ms(now, tz_offset) else {
                    break;
                };
                let wait_ms = next_ms.saturating_sub(now);
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = tokio::time::sleep(Duration::from_millis(wait_ms)) => {
                        if let Some(ref endpoint) = cloud_endpoint {
                            let _ = ctx
                                .http
                                .client()
                                .post(endpoint)
                                .json(&json!({
                                    "trigger_id": instance.id,
                                    "expression": expr,
                                    "fired_at_ms": next_ms,
                                }))
                                .send()
                                .await;
                        }
                        let ev = TriggerEvent {
                            trigger_id: instance.id.clone(),
                            kind: instance.kind.clone(),
                            prompt: instance.prompt.clone(),
                            preset: instance.preset.clone(),
                            extra_decorators: instance.extra_decorators.clone(),
                            metadata: json!({
                                "expression": expr,
                                "scheduled_ms": next_ms,
                            }),
                            timestamp_ms: current_epoch_ms(),
                        };
                        if event_tx.send(ev).is_err() {
                            break;
                        }
                    }
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_parser_and_next_fire_evaluator_work_across_boundaries() {
        let every_5 = CronSchedule::parse("*/5 * * * *").expect("every 5");
        // 2026-01-01 00:00:00 UTC = 1767225600000 ms (Thursday, dow=4)
        let base_ms = 1_767_225_600_000_u64;
        assert!(every_5.matches_epoch_ms(base_ms, 0));
        assert_eq!(
            every_5.next_fire_after_ms(base_ms + 1_000, 0),
            Some(base_ms + 300_000)
        );

        let weekday_morning = CronSchedule::parse("30 9 * * 1-5").expect("weekday");
        // Thursday 2026-01-01 09:30:00 UTC
        let thu_0930 = base_ms + (9 * 3600 + 30 * 60) * 1_000;
        assert!(weekday_morning.matches_epoch_ms(thu_0930, 0));
        // Saturday 2026-01-03 09:30:00 UTC should NOT match
        let sat_0930 = thu_0930 + 2 * 86_400_000;
        assert!(!weekday_morning.matches_epoch_ms(sat_0930, 0));

        assert!(CronSchedule::parse("* * *").is_err());
        assert!(CronSchedule::parse("61 * * * *").is_err());
        assert!(CronSchedule::parse("*/0 * * * *").is_err());
    }
}
