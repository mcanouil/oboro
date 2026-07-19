//! Reconciliation of spans produced by different detection layers.

use super::Span;

/// Resolves overlapping detections into a disjoint, ordered set.
///
/// Longer spans win, because a partial match is usually a fragment of a
/// larger entity: the phone recogniser sees a digit run inside an IBAN, and
/// keeping the IBAN redacts strictly more. Ties break on confidence, then on
/// the earlier start, so the result does not depend on detector ordering.
#[must_use]
pub fn resolve(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by(|a, b| {
        b.len()
            .cmp(&a.len())
            .then(
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.start.cmp(&b.start))
            .then(a.kind.cmp(&b.kind))
    });

    let mut kept: Vec<Span> = Vec::with_capacity(spans.len());
    for span in spans {
        if span.is_empty() {
            continue;
        }
        if !kept.iter().any(|existing| existing.overlaps(&span)) {
            kept.push(span);
        }
    }

    kept.sort_by_key(|span| span.start);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::EntityKind;

    #[test]
    fn longer_span_wins_over_contained_span() {
        let iban = Span::new(0, 27, EntityKind::Iban, "FR1420041010050500013M02606");
        let phone = Span::new(4, 15, EntityKind::Phone, "20041010050");
        let resolved = resolve(vec![phone, iban.clone()]);
        assert_eq!(resolved, [iban]);
    }

    #[test]
    fn disjoint_spans_are_all_kept_in_source_order() {
        let first = Span::new(10, 20, EntityKind::Email, "a@example.com");
        let second = Span::new(0, 5, EntityKind::Phone, "0612345678");
        let resolved = resolve(vec![first.clone(), second.clone()]);
        assert_eq!(resolved, [second, first]);
    }

    #[test]
    fn adjacent_spans_both_survive() {
        let first = Span::new(0, 5, EntityKind::Person, "Jean ");
        let second = Span::new(5, 11, EntityKind::Person, "Dupont");
        let resolved = resolve(vec![first, second]);
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn equal_length_overlap_breaks_on_confidence() {
        let low = Span {
            confidence: 0.4,
            ..Span::new(0, 6, EntityKind::Person, "Dupont")
        };
        let high = Span {
            confidence: 0.9,
            ..Span::new(2, 8, EntityKind::Organisation, "pont S")
        };
        let resolved = resolve(vec![low, high.clone()]);
        assert_eq!(resolved, [high]);
    }

    #[test]
    fn empty_spans_are_discarded() {
        let empty = Span::new(3, 3, EntityKind::Email, "");
        assert!(resolve(vec![empty]).is_empty());
    }

    #[test]
    fn resolving_nothing_yields_nothing() {
        assert!(resolve(Vec::new()).is_empty());
    }
}
