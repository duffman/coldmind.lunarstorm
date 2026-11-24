//! Allocation-free template scanning. Braces inside expressions are not supported;
//! double braces escape literal braces. Expressions use the explicit evaluator.
use super::DebugError;
/// One piece of a logpoint template. Its payloads are read by engine adapters only.
#[cfg_attr(not(feature = "quickjs-ng"), allow(dead_code))]
pub(crate) enum LogPart<'a> {
    Text(&'a [u16]),
    Brace(u16),
    Expression(&'a [u16]),
}
pub(crate) struct LogParts<'a> {
    units: &'a [u16],
    offset: usize,
}
impl<'a> LogParts<'a> {
    pub(crate) fn new(units: &'a [u16]) -> Self {
        Self { units, offset: 0 }
    }
}
impl<'a> Iterator for LogParts<'a> {
    type Item = Result<LogPart<'a>, DebugError>;
    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        let &unit = self.units.get(start)?;
        if unit == 123 || unit == 125 {
            if self.units.get(start + 1) == Some(&unit) {
                self.offset += 2;
                return Some(Ok(LogPart::Brace(unit)));
            }
            if unit == 123
                && let Some(end) = self.units[start + 1..]
                    .iter()
                    .position(|u| *u == 123 || *u == 125)
                    .map(|i| start + 1 + i)
                && self.units[end] == 125
                && self.units[start + 1..end]
                    .iter()
                    .any(|u| !matches!(*u, 9 | 10 | 13 | 32))
            {
                self.offset = end + 1;
                return Some(Ok(LogPart::Expression(&self.units[start + 1..end])));
            }
            self.offset = self.units.len();
            return Some(Err(DebugError::InvalidRequest));
        }
        self.offset = self.units[start..]
            .iter()
            .position(|u| *u == 123 || *u == 125)
            .map_or(self.units.len(), |i| start + i);
        Some(Ok(LogPart::Text(&self.units[start..self.offset])))
    }
}
pub(crate) fn validate(units: &[u16], expression_limit: usize) -> Result<(), DebugError> {
    for part in LogParts::new(units) {
        if let LogPart::Expression(expression) = part?
            && expression.len() > expression_limit
        {
            return Err(DebugError::Capacity);
        }
    }
    Ok(())
}
