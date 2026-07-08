use napi::bindgen_prelude::*;
use napi_derive::napi;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use uuid::Uuid;

static APP_STATE: Lazy<Mutex<RoomState>> = Lazy::new(|| Mutex::new(RoomState::new()));

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerSummary {
  id: String,
  label: String,
  session_role: String,
  ble_role: String,
  trust_status: String,
  connected: bool,
  last_seen_iso: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalculationEntry {
  id: String,
  origin_device_id: String,
  expression: String,
  result: String,
  trusted: bool,
  created_at_iso: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RoomState {
  local_device_id: String,
  room_id: Option<String>,
  room_name: Option<String>,
  session_role: Option<String>,
  ble_role: Option<String>,
  scanning: bool,
  advertising: bool,
  peers: Vec<PeerSummary>,
  history: Vec<CalculationEntry>,
}

impl RoomState {
  fn new() -> Self {
    Self {
      local_device_id: format!("native-{}", Uuid::new_v4()),
      room_id: None,
      room_name: None,
      session_role: None,
      ble_role: None,
      scanning: false,
      advertising: false,
      peers: Vec::new(),
      history: Vec::new(),
    }
  }
}

#[napi(object)]
pub struct CreateRoomRequest {
  pub room_name: String,
}

#[napi(object)]
pub struct StartAdvertisingRequest {
  pub room_code: String,
}

#[napi(object)]
pub struct ConnectGuestRequest {
  pub peer_id: String,
}

#[napi(object)]
pub struct SubmitCalculationRequest {
  pub expression: String,
}

#[napi]
pub fn get_state() -> Result<Value> {
  with_state_json(|state| Ok(state.clone()))
}

#[napi]
pub fn create_room(request: CreateRoomRequest) -> Result<Value> {
  with_state_json(|state| {
    let room_name = trim_or_default(&request.room_name, "Calculator Room");
    state.room_id = Some(format!("room-{}", Uuid::new_v4().simple().to_string()[..8].to_string()));
    state.room_name = Some(room_name);
    state.session_role = Some("host".to_string());
    state.ble_role = Some("central".to_string());
    state.scanning = false;
    state.advertising = false;
    Ok(state.clone())
  })
}

#[napi]
pub fn start_scanning() -> Result<Value> {
  with_state_json(|state| {
    state.scanning = true;
    state.advertising = false;

    if state.peers.is_empty() {
      state.peers.push(PeerSummary {
        id: "guest-native-linux".to_string(),
        label: "Linux Calculator".to_string(),
        session_role: "guest".to_string(),
        ble_role: "peripheral".to_string(),
        trust_status: "pending".to_string(),
        connected: false,
        last_seen_iso: now_iso(),
      });
    }

    Ok(state.clone())
  })
}

#[napi]
pub fn connect_guest(request: ConnectGuestRequest) -> Result<Value> {
  with_state_json(|state| {
    for peer in &mut state.peers {
      if peer.id == request.peer_id {
        peer.connected = true;
        peer.trust_status = "trusted".to_string();
        peer.last_seen_iso = now_iso();
      }
    }

    Ok(state.clone())
  })
}

#[napi]
pub fn start_advertising(request: StartAdvertisingRequest) -> Result<Value> {
  with_state_json(|state| {
    let room_code = request.room_code.trim().to_string();
    state.room_id = if room_code.is_empty() { None } else { Some(room_code.clone()) };
    state.room_name = state.room_id.as_ref().map(|code| format!("Join {}", code));
    state.session_role = Some("guest".to_string());
    state.ble_role = Some("peripheral".to_string());
    state.scanning = false;
    state.advertising = true;
    Ok(state.clone())
  })
}

#[napi]
pub fn accept_host_connection() -> Result<Value> {
  with_state_json(|state| {
    state.advertising = false;
    state.peers = vec![PeerSummary {
      id: "host-native-mac".to_string(),
      label: "Mac Host".to_string(),
      session_role: "host".to_string(),
      ble_role: "central".to_string(),
      trust_status: "trusted".to_string(),
      connected: true,
      last_seen_iso: now_iso(),
    }];

    Ok(state.clone())
  })
}

#[napi]
pub fn submit_calculation(request: SubmitCalculationRequest) -> Result<Value> {
  with_state_json(|state| {
    let expression = request.expression.trim().to_string();
    if expression.is_empty() {
      return Ok(state.clone());
    }

    state.history.insert(
      0,
      CalculationEntry {
        id: Uuid::new_v4().to_string(),
        origin_device_id: state.local_device_id.clone(),
        result: calculate_expression(&expression),
        expression,
        trusted: true,
        created_at_iso: now_iso(),
      },
    );

    Ok(state.clone())
  })
}

fn with_state_json<T>(mut action: impl FnMut(&mut RoomState) -> Result<T>) -> Result<Value>
where
  T: Serialize,
{
  let mut state = APP_STATE
    .lock()
    .map_err(|_| Error::from_reason("Native calculator state lock was poisoned"))?;
  let value = action(&mut state)?;
  serde_json::to_value(value)
    .map_err(|error| Error::from_reason(format!("Failed to serialize native response: {error}")))
}

fn trim_or_default(value: &str, default_value: &str) -> String {
  let trimmed = value.trim();
  if trimmed.is_empty() {
    default_value.to_string()
  } else {
    trimmed.to_string()
  }
}

fn now_iso() -> String {
  OffsetDateTime::now_utc()
    .format(&Rfc3339)
    .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn calculate_expression(expression: &str) -> String {
  match parse_expression(expression) {
    Some(value) if value.is_finite() => {
      let rounded = (value * 100_000_000.0).round() / 100_000_000.0;
      format!("{rounded}")
    }
    _ => "Invalid expression".to_string(),
  }
}

fn parse_expression(expression: &str) -> Option<f64> {
  let sanitized: String = expression.chars().filter(|ch| !ch.is_whitespace()).collect();
  if sanitized.is_empty() {
    return None;
  }

  let tokens = tokenize(&sanitized)?;
  let mut values: Vec<f64> = Vec::new();
  let mut ops: Vec<char> = Vec::new();

  for token in tokens {
    match token {
      Token::Number(value) => values.push(value),
      Token::Op(op) => {
        while ops
          .last()
          .is_some_and(|existing| precedence(*existing) >= precedence(op))
        {
          apply_op(&mut values, ops.pop()?)?;
        }
        ops.push(op);
      }
    }
  }

  while let Some(op) = ops.pop() {
    apply_op(&mut values, op)?;
  }

  if values.len() == 1 {
    values.pop()
  } else {
    None
  }
}

#[derive(Debug)]
enum Token {
  Number(f64),
  Op(char),
}

fn tokenize(input: &str) -> Option<Vec<Token>> {
  let mut tokens = Vec::new();
  let mut chars = input.chars().peekable();
  let mut expects_number = true;

  while let Some(ch) = chars.peek().copied() {
    if ch.is_ascii_digit() || ch == '.' || (ch == '-' && expects_number) {
      let mut value = String::new();
      if ch == '-' {
        value.push(chars.next()?);
      }

      while let Some(next) = chars.peek().copied() {
        if next.is_ascii_digit() || next == '.' {
          value.push(chars.next()?);
        } else {
          break;
        }
      }

      tokens.push(Token::Number(value.parse().ok()?));
      expects_number = false;
      continue;
    }

    if matches!(ch, '+' | '-' | '*' | '/' | '%') && !expects_number {
      tokens.push(Token::Op(chars.next()?));
      expects_number = true;
      continue;
    }

    return None;
  }

  if expects_number {
    return None;
  }

  Some(tokens)
}

fn precedence(op: char) -> u8 {
  match op {
    '+' | '-' => 1,
    '*' | '/' | '%' => 2,
    _ => 0,
  }
}

fn apply_op(values: &mut Vec<f64>, op: char) -> Option<()> {
  let right = values.pop()?;
  let left = values.pop()?;
  let result = match op {
    '+' => left + right,
    '-' => left - right,
    '*' => left * right,
    '/' if right != 0.0 => left / right,
    '%' if right != 0.0 => left % right,
    _ => return None,
  };
  values.push(result);
  Some(())
}

#[cfg(test)]
mod tests {
  use super::calculate_expression;

  #[test]
  fn evaluates_operator_precedence() {
    assert_eq!(calculate_expression("7 + 5 * 2"), "17");
  }

  #[test]
  fn rejects_invalid_expression() {
    assert_eq!(calculate_expression("7 + nope"), "Invalid expression");
  }

  #[test]
  fn evaluates_modulo() {
    assert_eq!(calculate_expression("10 % 4"), "2");
  }
}
