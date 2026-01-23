pub fn tag_name(tag: &[String]) -> Option<&str> {
    tag.first().map(String::as_str)
}

pub fn tag_value(tag: &[String]) -> Option<&str> {
    tag.get(1).map(String::as_str)
}

pub fn tag_values(tag: &[String]) -> &[String] {
    if tag.len() <= 1 { &[] } else { &tag[1..] }
}

pub fn collect_tag_values(tags: &[Vec<String>], name: &str) -> Vec<String> {
    let mut values = Vec::new();
    for tag in tags {
        if tag.first().map(|t| t == name).unwrap_or(false) {
            values.extend(tag.iter().skip(1).cloned());
        }
    }
    values
}

pub fn collect_tag_values_unique(tags: &[Vec<String>], name: &str) -> Vec<String> {
    let mut values = Vec::new();
    for tag in tags {
        if tag.first().map(|t| t == name).unwrap_or(false) {
            extend_unique(&mut values, tag_values(tag));
        }
    }
    values
}

pub fn push_unique(target: &mut Vec<String>, value: &str) {
    if !target.iter().any(|item| item == value) {
        target.push(value.to_string());
    }
}

pub fn extend_unique(target: &mut Vec<String>, values: &[String]) {
    for value in values {
        push_unique(target, value);
    }
}

pub fn join_tag_values(kind: &str, values: &[String]) -> Vec<String> {
    let mut tag = Vec::with_capacity(values.len() + 1);
    tag.push(kind.to_string());
    tag.extend(values.iter().cloned());
    tag
}

#[cfg(test)]
mod tests {
    use super::collect_tag_values;
    use super::collect_tag_values_unique;
    use super::join_tag_values;
    use super::tag_name;
    use super::tag_value;
    use super::tag_values;

    #[test]
    fn tag_helpers_handle_empty_tags() {
        let tag: Vec<String> = Vec::new();
        assert!(tag_name(&tag).is_none());
        assert!(tag_value(&tag).is_none());
        assert!(tag_values(&tag).is_empty());
    }

    #[test]
    fn collect_tag_values_merges_across_tags() {
        let tags = vec![
            vec!["clone".to_string(), "a".to_string(), "b".to_string()],
            vec!["relays".to_string(), "r1".to_string()],
            vec!["clone".to_string(), "c".to_string()],
        ];

        let values = collect_tag_values(&tags, "clone");
        assert_eq!(values, vec!["a", "b", "c"]);
    }

    #[test]
    fn collect_tag_values_unique_dedupes() {
        let tags = vec![
            vec!["clone".to_string(), "a".to_string(), "b".to_string()],
            vec!["clone".to_string(), "b".to_string(), "c".to_string()],
        ];

        let values = collect_tag_values_unique(&tags, "clone");
        assert_eq!(values, vec!["a", "b", "c"]);
    }

    #[test]
    fn join_tag_values_formats_tag() {
        let tag = join_tag_values(
            "relays",
            &[
                "wss://relay.example".to_string(),
                "wss://relay.two".to_string(),
            ],
        );
        assert_eq!(
            tag,
            vec![
                "relays".to_string(),
                "wss://relay.example".to_string(),
                "wss://relay.two".to_string()
            ]
        );
    }
}
