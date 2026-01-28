use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateWindow {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resets_at: Option<OffsetDateTime>,
    pub reset_description: Option<String>,
}

impl RateWindow {
    pub fn remaining_percent(&self) -> f64 {
        let remaining = 100.0 - self.used_percent;
        remaining.max(0.0).min(100.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderIdentitySnapshot {
    #[serde(rename = "providerID")]
    pub provider_id: Option<String>,
    pub account_email: Option<String>,
    pub account_organization: Option<String>,
    pub login_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCostSnapshot {
    pub used: f64,
    pub limit: f64,
    pub currency_code: String,
    pub period: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resets_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
    pub tertiary: Option<RateWindow>,
    pub provider_cost: Option<ProviderCostSnapshot>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub identity: Option<ProviderIdentitySnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub email: Option<String>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreditEvent {
    pub id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub date: OffsetDateTime,
    pub service: String,
    pub credits_used: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreditsSnapshot {
    pub remaining: f64,
    pub events: Vec<CreditEvent>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIDashboardSnapshot {
    pub signed_in_email: Option<String>,
    pub code_review_remaining_percent: Option<f64>,
    pub credit_events: Vec<CreditEvent>,
    pub daily_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
    pub usage_breakdown: Vec<OpenAIDashboardDailyBreakdown>,
    #[serde(rename = "creditsPurchaseURL")]
    pub credits_purchase_url: Option<String>,
    pub primary_limit: Option<RateWindow>,
    pub secondary_limit: Option<RateWindow>,
    pub credits_remaining: Option<f64>,
    pub account_plan: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIDashboardDailyBreakdown {
    pub day: String,
    pub services: Vec<OpenAIDashboardServiceUsage>,
    pub total_credits_used: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIDashboardServiceUsage {
    pub service: String,
    pub credits_used: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityPlanInfoSummary {
    pub plan_name: Option<String>,
    pub plan_display_name: Option<String>,
    pub display_name: Option<String>,
    pub product_name: Option<String>,
    pub plan_short_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatusPayload {
    pub indicator: ProviderStatusIndicator,
    pub description: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderStatusIndicator {
    None,
    Minor,
    Major,
    Critical,
    Maintenance,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderErrorPayload {
    pub code: i32,
    pub message: String,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPayload {
    pub provider: String,
    pub account: Option<String>,
    pub version: Option<String>,
    pub source: String,
    pub status: Option<ProviderStatusPayload>,
    pub usage: Option<UsageSnapshot>,
    pub credits: Option<CreditsSnapshot>,
    pub antigravity_plan_info: Option<AntigravityPlanInfoSummary>,
    pub openai_dashboard: Option<OpenAIDashboardSnapshot>,
    pub error: Option<ProviderErrorPayload>,
}
