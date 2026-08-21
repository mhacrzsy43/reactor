use std::collections::HashMap;

use quick_xml::{Reader, events::Event};
use reactor_protocol::{Coordinate, Platform, Selector};
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorStability {
    Stable,
    Contextual,
    Brittle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    #[must_use]
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x <= self.x + self.width && y <= self.y + self.height
    }

    #[must_use]
    pub fn area(&self) -> f64 {
        self.width * self.height
    }

    #[must_use]
    pub fn center(&self) -> Coordinate {
        Coordinate {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectorCandidate {
    pub strategy: String,
    pub label: String,
    pub selector: Selector,
    pub score: u8,
    pub stability: SelectorStability,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct InspectorElement {
    pub key: String,
    pub depth: u32,
    pub text: Option<String>,
    pub accessibility_text: Option<String>,
    pub resource_id: Option<String>,
    pub package_name: Option<String>,
    pub bounds: Bounds,
    pub enabled: bool,
    pub clickable: bool,
    pub editable: bool,
    pub password: bool,
    pub focused: bool,
    pub class_name: Option<String>,
    pub candidates: Vec<SelectorCandidate>,
}

#[derive(Debug, Error)]
pub enum InspectorError {
    #[error("invalid Android hierarchy: {0}")]
    AndroidXml(String),
    #[error("invalid iOS hierarchy: {0}")]
    IosCsv(String),
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
struct RawElement {
    key: String,
    depth: u32,
    text: Option<String>,
    accessibility_text: Option<String>,
    resource_id: Option<String>,
    package_name: Option<String>,
    bounds: Bounds,
    enabled: bool,
    clickable: bool,
    editable: bool,
    password: bool,
    focused: bool,
    class_name: Option<String>,
}

/// Parses a platform accessibility hierarchy and attaches deterministic selector candidates.
///
/// # Errors
///
/// Returns an error when the hierarchy cannot be parsed.
pub fn inspect_hierarchy(
    platform: Platform,
    hierarchy: &str,
) -> Result<Vec<InspectorElement>, InspectorError> {
    let raw = match platform {
        Platform::Android => parse_android(hierarchy)?,
        Platform::Ios => parse_ios(hierarchy)?,
    };
    Ok(score_elements(raw))
}

#[must_use]
pub fn hit_test(elements: &[InspectorElement], x: f64, y: f64) -> Option<&InspectorElement> {
    let matching = elements
        .iter()
        .filter(|element| element.bounds.contains(x, y) && element.bounds.area() > 0.0)
        .collect::<Vec<_>>();
    matching
        .iter()
        .copied()
        .filter(|element| (element.clickable || element.editable) && element.enabled)
        .min_by(|left, right| left.bounds.area().total_cmp(&right.bounds.area()))
        .or_else(|| {
            matching
                .into_iter()
                .min_by(|left, right| left.bounds.area().total_cmp(&right.bounds.area()))
        })
}

fn parse_android(hierarchy: &str) -> Result<Vec<RawElement>, InspectorError> {
    let mut reader = Reader::from_str(hierarchy);
    reader.config_mut().trim_text(true);
    let mut output = Vec::new();
    let mut depth = 0_u32;
    let mut sequence = 0_u32;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                if event.name().as_ref() == b"node" {
                    if let Some(element) = android_node(&event, depth, sequence)? {
                        output.push(element);
                        sequence += 1;
                    }
                    depth = depth.saturating_add(1);
                }
            }
            Ok(Event::Empty(event)) if event.name().as_ref() == b"node" => {
                if let Some(element) = android_node(&event, depth, sequence)? {
                    output.push(element);
                    sequence += 1;
                }
            }
            Ok(Event::End(event)) if event.name().as_ref() == b"node" => {
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(InspectorError::AndroidXml(error.to_string())),
        }
    }
    Ok(output)
}

fn android_node(
    event: &quick_xml::events::BytesStart<'_>,
    depth: u32,
    sequence: u32,
) -> Result<Option<RawElement>, InspectorError> {
    let mut attributes = HashMap::new();
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| InspectorError::AndroidXml(error.to_string()))?;
        let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|error| InspectorError::AndroidXml(error.to_string()))?
            .into_owned();
        attributes.insert(key, value);
    }
    let Some(bounds) = attributes
        .get("bounds")
        .and_then(|value| parse_bounds(value))
    else {
        return Ok(None);
    };
    Ok(Some(RawElement {
        key: format!("android-{sequence}"),
        depth,
        text: non_empty(attributes.get("text")),
        accessibility_text: non_empty(attributes.get("content-desc")),
        resource_id: non_empty(attributes.get("resource-id")),
        package_name: non_empty(attributes.get("package")),
        bounds,
        enabled: bool_attribute(&attributes, "enabled", true),
        clickable: bool_attribute(&attributes, "clickable", false),
        editable: bool_attribute(&attributes, "editable", false)
            || attributes
                .get("class")
                .is_some_and(|value| value.contains("EditText")),
        password: bool_attribute(&attributes, "password", false),
        focused: bool_attribute(&attributes, "focused", false),
        class_name: non_empty(attributes.get("class")),
    }))
}

fn parse_ios(hierarchy: &str) -> Result<Vec<RawElement>, InspectorError> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(hierarchy.as_bytes());
    let mut output = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|error| InspectorError::IosCsv(error.to_string()))?;
        let key = record.get(0).unwrap_or_default().trim();
        let depth = record
            .get(1)
            .unwrap_or_default()
            .trim()
            .parse::<u32>()
            .unwrap_or_default();
        let attributes = parse_ios_attributes(record.get(2).unwrap_or_default());
        let Some(bounds) = attributes
            .get("bounds")
            .and_then(|value| parse_bounds(value))
        else {
            continue;
        };
        output.push(RawElement {
            key: format!("ios-{key}"),
            depth,
            text: non_empty(attributes.get("text")),
            accessibility_text: non_empty(attributes.get("accessibilityText")),
            resource_id: non_empty(attributes.get("resource-id")),
            package_name: None,
            bounds,
            enabled: bool_attribute(&attributes, "enabled", true),
            clickable: true,
            editable: bool_attribute(&attributes, "editable", false)
                || attributes
                    .get("class")
                    .or_else(|| attributes.get("type"))
                    .is_some_and(|value| {
                        value.contains("TextField") || value.contains("SecureTextField")
                    }),
            password: bool_attribute(&attributes, "password", false)
                || attributes
                    .get("class")
                    .or_else(|| attributes.get("type"))
                    .is_some_and(|value| value.contains("SecureTextField")),
            focused: bool_attribute(&attributes, "focused", false),
            class_name: non_empty(attributes.get("class").or_else(|| attributes.get("type"))),
        });
    }
    Ok(output)
}

fn parse_ios_attributes(input: &str) -> HashMap<String, String> {
    input
        .split("; ")
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

fn parse_bounds(input: &str) -> Option<Bounds> {
    let expression = Regex::new(
        r"\[(-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?)\]\[(-?\d+(?:\.\d+)?),(-?\d+(?:\.\d+)?)\]",
    )
    .ok()?;
    let captures = expression.captures(input)?;
    let x1 = captures.get(1)?.as_str().parse::<f64>().ok()?;
    let y1 = captures.get(2)?.as_str().parse::<f64>().ok()?;
    let x2 = captures.get(3)?.as_str().parse::<f64>().ok()?;
    let y2 = captures.get(4)?.as_str().parse::<f64>().ok()?;
    Some(Bounds {
        x: x1,
        y: y1,
        width: (x2 - x1).max(0.0),
        height: (y2 - y1).max(0.0),
    })
}

fn non_empty(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn bool_attribute(attributes: &HashMap<String, String>, key: &str, default: bool) -> bool {
    attributes
        .get(key)
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(default)
}

fn score_elements(raw: Vec<RawElement>) -> Vec<InspectorElement> {
    let mut resource_counts = HashMap::<String, usize>::new();
    let mut text_counts = HashMap::<String, usize>::new();
    let mut clickable_text_counts = HashMap::<String, usize>::new();
    for element in &raw {
        if let Some(resource_id) = &element.resource_id {
            *resource_counts.entry(resource_id.clone()).or_default() += 1;
        }
        if let Some(text) = selector_text(element) {
            *text_counts.entry(text.clone()).or_default() += 1;
            if (element.clickable || element.editable) && element.enabled {
                *clickable_text_counts.entry(text.clone()).or_default() += 1;
            }
        }
    }
    let mut text_occurrences = HashMap::<String, u32>::new();
    raw.into_iter()
        .map(|element| {
            let text_index = selector_text(&element).map(|text| {
                let entry = text_occurrences.entry(text.clone()).or_default();
                let index = *entry;
                *entry += 1;
                index
            });
            let candidates = selector_candidates(
                &element,
                &resource_counts,
                &text_counts,
                &clickable_text_counts,
                text_index,
            );
            InspectorElement {
                key: element.key,
                depth: element.depth,
                text: element.text,
                accessibility_text: element.accessibility_text,
                resource_id: element.resource_id,
                package_name: element.package_name,
                bounds: element.bounds,
                enabled: element.enabled,
                clickable: element.clickable,
                editable: element.editable,
                password: element.password,
                focused: element.focused,
                class_name: element.class_name,
                candidates,
            }
        })
        .collect()
}

fn selector_text(element: &RawElement) -> Option<&String> {
    element
        .text
        .as_ref()
        .or(element.accessibility_text.as_ref())
}

fn selector_candidates(
    element: &RawElement,
    resource_counts: &HashMap<String, usize>,
    text_counts: &HashMap<String, usize>,
    clickable_text_counts: &HashMap<String, usize>,
    text_index: Option<u32>,
) -> Vec<SelectorCandidate> {
    let mut candidates = Vec::new();
    if let Some(resource_id) = &element.resource_id {
        let unique = resource_counts.get(resource_id) == Some(&1);
        candidates.push(SelectorCandidate {
            strategy: "id".to_owned(),
            label: resource_id.clone(),
            selector: Selector {
                accessibility_id: Some(resource_id.clone()),
                ..Selector::default()
            },
            score: if unique { 96 } else { 72 },
            stability: if unique {
                SelectorStability::Stable
            } else {
                SelectorStability::Contextual
            },
            reason: if unique {
                "当前页面唯一的资源 ID".to_owned()
            } else {
                "资源 ID 重复，需要页面上下文".to_owned()
            },
        });
    }
    if let Some(text) = selector_text(element) {
        let unique_interactive =
            (element.clickable || element.editable) && clickable_text_counts.get(text) == Some(&1);
        let unique = unique_interactive || text_counts.get(text) == Some(&1);
        let index = (!unique).then_some(text_index).flatten();
        candidates.push(SelectorCandidate {
            strategy: "text".to_owned(),
            label: text.clone(),
            selector: Selector {
                text: Some(text.clone()),
                index,
                ..Selector::default()
            },
            score: if unique { 82 } else { 58 },
            stability: if unique {
                SelectorStability::Stable
            } else {
                SelectorStability::Contextual
            },
            reason: if unique_interactive {
                "当前页面唯一的可交互文本/无障碍标签".to_owned()
            } else if unique {
                "当前页面唯一的可见/无障碍文本".to_owned()
            } else {
                format!(
                    "文本在当前页面重复；已绑定同名控件第 {} 个",
                    index.map_or(0, |value| value + 1)
                )
            },
        });
    }
    let coordinate = element.bounds.center();
    candidates.push(SelectorCandidate {
        strategy: "coordinate".to_owned(),
        label: format!("{:.0}, {:.0}", coordinate.x, coordinate.y),
        selector: Selector {
            coordinate: Some(coordinate),
            ..Selector::default()
        },
        score: 20,
        stability: SelectorStability::Brittle,
        reason: "仅在 UI 树无法提供稳定语义时使用；布局变化会失效".to_owned(),
    });
    candidates.sort_by(|left, right| right.score.cmp(&left.score));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_resource_id_beats_text_and_coordinate() {
        let hierarchy = r#"<?xml version="1.0"?><hierarchy><node text="课程" resource-id="com.example:id/course" package="com.example" content-desc="" clickable="true" enabled="true" bounds="[10,20][210,100]" /></hierarchy>"#;
        let elements = inspect_hierarchy(Platform::Android, hierarchy).unwrap();
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].candidates[0].strategy, "id");
        assert_eq!(elements[0].candidates[0].score, 96);
        assert_eq!(elements[0].package_name.as_deref(), Some("com.example"));
        assert_eq!(
            elements[0].candidates.last().unwrap().strategy,
            "coordinate"
        );
        assert_eq!(hit_test(&elements, 50.0, 50.0).unwrap().key, "android-0");
    }

    #[test]
    fn android_content_description_maps_to_maestro_text() {
        let hierarchy = r#"<hierarchy><node text="" resource-id="" content-desc="Open list" clickable="true" enabled="true" bounds="[0,0][100,80]" /></hierarchy>"#;
        let elements = inspect_hierarchy(Platform::Android, hierarchy).unwrap();
        assert_eq!(
            elements[0].candidates[0].selector.text.as_deref(),
            Some("Open list")
        );
    }

    #[test]
    fn ios_compact_hierarchy_preserves_commas_and_scores_resource_id() {
        let hierarchy = "element_num,depth,attributes,parent_num\n7,2,\"accessibilityText=课程, 列表; resource-id=COURSES; bounds=[16,40][300,100]; enabled=true\",1\n";
        let elements = inspect_hierarchy(Platform::Ios, hierarchy).unwrap();
        assert_eq!(
            elements[0].accessibility_text.as_deref(),
            Some("课程, 列表")
        );
        assert_eq!(elements[0].candidates[0].label, "COURSES");
        assert!((elements[0].bounds.width - 284.0).abs() < f64::EPSILON);
    }

    #[test]
    fn duplicate_text_is_contextual_and_hit_test_prefers_smallest_element() {
        let hierarchy = r#"<hierarchy><node text="List" bounds="[0,0][300,300]"><node text="List" bounds="[20,20][100,80]" /></node></hierarchy>"#;
        let elements = inspect_hierarchy(Platform::Android, hierarchy).unwrap();
        let text = &elements[0].candidates[0];
        assert_eq!(text.stability, SelectorStability::Contextual);
        assert_eq!(text.selector.index, Some(0));
        assert_eq!(elements[1].candidates[0].selector.index, Some(1));
        assert!(
            (hit_test(&elements, 50.0, 50.0).unwrap().bounds.width - 80.0).abs() < f64::EPSILON
        );
    }

    #[test]
    fn duplicate_interactive_labels_keep_their_accessibility_tree_index() {
        let hierarchy = r#"<hierarchy>
          <node text="Sign in" clickable="false" enabled="true" bounds="[0,0][100,20]" />
          <node text="" content-desc="Sign in" clickable="true" enabled="true" bounds="[0,30][100,70]" />
          <node text="Sign in" clickable="false" enabled="true" bounds="[20,40][80,60]" />
          <node text="" content-desc="Sign in" clickable="true" enabled="true" bounds="[0,80][100,120]" />
          <node text="Sign in" clickable="false" enabled="true" bounds="[20,90][80,110]" />
        </hierarchy>"#;
        let elements = inspect_hierarchy(Platform::Android, hierarchy).unwrap();
        let submit = hit_test(&elements, 50.0, 100.0).unwrap();
        let selector = submit
            .candidates
            .iter()
            .find(|candidate| candidate.strategy == "text")
            .unwrap();
        assert_eq!(selector.selector.text.as_deref(), Some("Sign in"));
        assert_eq!(selector.selector.index, Some(3));
        assert!(selector.reason.contains("第 4 个"));
    }

    #[test]
    fn hit_test_prefers_clickable_parent_and_scores_unique_interactive_label() {
        let hierarchy = r#"<hierarchy><node text="" content-desc="List scenario" clickable="true" enabled="true" bounds="[0,0][300,100]"><node text="List scenario" content-desc="" clickable="false" enabled="true" bounds="[80,20][220,80]" /></node></hierarchy>"#;
        let elements = inspect_hierarchy(Platform::Android, hierarchy).unwrap();
        let hit = hit_test(&elements, 100.0, 50.0).unwrap();
        assert!(hit.clickable);
        assert_eq!(hit.candidates[0].score, 82);
        assert_eq!(hit.candidates[0].stability, SelectorStability::Stable);
        assert!(hit.candidates[0].reason.contains("可交互"));
    }

    #[test]
    fn android_edit_text_is_inspectable_even_when_not_clickable() {
        let hierarchy = r#"<hierarchy><node text="" resource-id="com.demo:id/password" class="android.widget.EditText" password="true" focused="true" clickable="false" enabled="true" bounds="[20,40][300,100]" /></hierarchy>"#;
        let elements = inspect_hierarchy(Platform::Android, hierarchy).unwrap();
        let input = hit_test(&elements, 80.0, 70.0).unwrap();
        assert!(input.editable);
        assert!(input.password);
        assert!(input.focused);
        assert_eq!(input.class_name.as_deref(), Some("android.widget.EditText"));
        assert_eq!(input.candidates[0].strategy, "id");
    }
}
