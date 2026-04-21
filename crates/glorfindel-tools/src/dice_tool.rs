use async_trait::async_trait;
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::Permission;
use rand::Rng;
use uuid::Uuid;

use crate::error::ToolError;
use crate::traits::Tool;

pub struct DiceRollTool;

/// Parse "XdY+Z" / "XdY-Z" / "dY" notation.
/// Returns (count, sides, modifier).
pub fn parse_notation(notation: &str) -> Option<(u32, u32, i32)> {
    let s = notation.trim().to_lowercase().replace(" ", "");

    // Split on 'd'
    let d_pos = s.find('d')?;
    let count_str = &s[..d_pos];
    let rest = &s[d_pos + 1..];

    let count: u32 = if count_str.is_empty() { 1 } else { count_str.parse().ok()? };
    if count == 0 || count > 100 { return None; }

    // Split rest on '+' or '-' for modifier
    let (sides_str, modifier): (&str, i32) = if let Some(pos) = rest.rfind('+') {
        (&rest[..pos], rest[pos + 1..].parse().ok()?)
    } else if let Some(pos) = rest.rfind('-') {
        (&rest[..pos], -rest[pos + 1..].parse::<i32>().ok()?)
    } else {
        (rest, 0)
    };

    let sides: u32 = sides_str.parse().ok()?;
    if sides < 2 || sides > 1000 { return None; }

    Some((count, sides, modifier))
}

#[async_trait]
impl Tool for DiceRollTool {
    fn name(&self) -> &str { "dice.roll" }

    fn description(&self) -> &str {
        "Roll dice using standard notation. Parameter: 'notation' (string, e.g. 'd20', '2d6+3', 'd20-1'). \
         Returns each die result and the total."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![] // no permission needed — it's dice
    }

    async fn execute(&self, task_id: Uuid, parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let notation = parameters
            .get("notation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("notation".into()))?;

        let (count, sides, modifier) = parse_notation(notation)
            .ok_or_else(|| ToolError::InvalidParameter(format!("cannot parse dice notation: '{notation}'")))?;

        let mut rng = rand::thread_rng();
        let rolls: Vec<u32> = (0..count).map(|_| rng.gen_range(1..=sides)).collect();
        let sum: u32 = rolls.iter().sum();
        let total = sum as i32 + modifier;

        let modifier_str = match modifier.cmp(&0) {
            std::cmp::Ordering::Greater => format!("+{modifier}"),
            std::cmp::Ordering::Less    => format!("{modifier}"),
            std::cmp::Ordering::Equal   => String::new(),
        };

        Ok(ToolResult::success(
            task_id,
            "dice.roll",
            serde_json::json!({
                "notation": notation,
                "rolls": rolls,
                "modifier": modifier,
                "total": total,
                "summary": format!("{notation} → [{}]{} = {}",
                    rolls.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", "),
                    modifier_str,
                    total
                ),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse() {
        assert_eq!(parse_notation("d20"), Some((1, 20, 0)));
        assert_eq!(parse_notation("2d6+3"), Some((2, 6, 3)));
        assert_eq!(parse_notation("d20-1"), Some((1, 20, -1)));
        assert_eq!(parse_notation("4d6"), Some((4, 6, 0)));
        assert_eq!(parse_notation("1d4+2"), Some((1, 4, 2)));
    }
}
