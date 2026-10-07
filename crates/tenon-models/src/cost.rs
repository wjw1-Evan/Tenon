//! 成本计算（设计方案 §11）：云端按价格表；本地模型「本地 · 0 成本」。

/// 价格表：每百万 token 美元单价。
#[derive(Debug, Clone, Default)]
pub struct PriceTable {
    /// model 名（小写精确匹配，前缀模糊回退）→ (input_usd_per_mtok, output_usd_per_mtok)
    rates: std::collections::BTreeMap<String, (f64, f64)>,
}

impl PriceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_rate(mut self, model: &str, input_per_mtok: f64, output_per_mtok: f64) -> Self {
        self.rates
            .insert(model.to_lowercase(), (input_per_mtok, output_per_mtok));
        self
    }

    fn rate_for(&self, model: &str) -> Option<(f64, f64)> {
        let m = model.to_lowercase();
        if let Some(r) = self.rates.get(&m) {
            return Some(*r);
        }
        // 前缀匹配（glm-4.6 → glm 系列）：键互为前缀时取最长匹配——
        // BTreeMap 字典序先到的短键（gpt-4）会截胡更特异的 gpt-4o，价差误报
        self.rates
            .iter()
            .filter(|(k, _)| m.starts_with(k.as_str()))
            .max_by_key(|(k, _)| k.len())
            .map(|(_, v)| *v)
    }
}

/// 计算一次调用的美元成本；未知模型按 0 计（宁少报不虚报）。
pub fn compute_cost(table: &PriceTable, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
    match table.rate_for(model) {
        Some((in_rate, out_rate)) => {
            input_tokens as f64 / 1_000_000.0 * in_rate
                + output_tokens as f64 / 1_000_000.0 * out_rate
        }
        None => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_from_price_table() {
        let table = PriceTable::new()
            .with_rate("glm-4.6", 0.6, 2.2)
            .with_rate("gpt-4o", 2.5, 10.0);
        let cost = compute_cost(&table, "glm-4.6", 1_000_000, 500_000);
        assert!((cost - (0.6 + 1.1)).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_costs_zero() {
        let table = PriceTable::new();
        assert_eq!(compute_cost(&table, "mystery-model", 1000, 1000), 0.0);
    }

    #[test]
    fn prefix_fallback_matches_family() {
        let table = PriceTable::new().with_rate("glm", 0.6, 2.2);
        let cost = compute_cost(&table, "glm-4.6-air", 1_000_000, 0);
        assert!((cost - 0.6).abs() < 1e-9);
    }

    #[test]
    fn prefix_fallback_prefers_most_specific_key() {
        // gpt-4 与 gpt-4o 互为前缀：gpt-4o-mini 必须命中更特异的 gpt-4o 价
        let table = PriceTable::new()
            .with_rate("gpt-4", 30.0, 60.0)
            .with_rate("gpt-4o", 2.5, 10.0);
        let cost = compute_cost(&table, "gpt-4o-mini", 1_000_000, 0);
        assert!((cost - 2.5).abs() < 1e-9, "应命中 gpt-4o 而非 gpt-4");
    }
}
