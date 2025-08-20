//! # Automotive Real-Time Metrics Module
//!
//! This module provides specialized metrics collection and evaluation for 
//! automotive real-time systems, focusing on:
//! - Hard deadline compliance (ASIL safety requirements)
//! - Deterministic timing validation  
//! - Safety-critical error tracking
//! - Automotive-specific performance analysis

use crate::cli::{AsilLevel, IpcMechanism};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Automotive-specific error types for safety-critical evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutomotiveError {
    /// Hard deadline was missed (latency exceeded limit)
    DeadlineMissed {
        latency_us: u64,
        deadline_us: u64,
        mechanism: IpcMechanism,
    },
    
    /// Safety error budget exceeded for ASIL level
    ErrorBudgetExceeded {
        error_count_ppm: u64,
        max_allowed_ppm: u64,
        asil_level: AsilLevel,
    },
    
    /// System entered failsafe mode due to critical failure
    SafetyCriticalFailure(String),
    
    /// Consecutive failures exceeded safety threshold
    ConsecutiveFailuresExceeded {
        count: u64,
        max_allowed: u64,
    },
    
    /// Timing jitter exceeded automotive requirements
    JitterExceeded {
        measured_jitter_us: u64,
        max_allowed_jitter_us: u64,
    },
}

/// Automotive application categories with different timing requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutomotiveApplication {
    /// Life-critical systems (ASIL-D): airbag, steering, brake-by-wire
    /// Requirement: <100μs, <0.1 PPM error rate
    LifeCritical,
    
    /// Safety-critical systems (ASIL-C): ESC, ABS, power steering  
    /// Requirement: <1ms, <1 PPM error rate
    SafetyCritical,
    
    /// Real-time control (ASIL-B): engine, transmission, suspension
    /// Requirement: <10ms, <10 PPM error rate  
    RealTimeControl,
    
    /// Comfort systems (ASIL-A): climate, seat, lighting
    /// Requirement: <100ms, <100 PPM error rate
    ComfortSystems,
    
    /// Infotainment (QM): navigation, entertainment, connectivity
    /// Requirement: <1s, best effort
    Infotainment,
    
    /// Diagnostic systems (QM): OBD, maintenance, logging
    /// Requirement: <10s, best effort  
    Diagnostics,
}

impl AutomotiveApplication {
    /// Get maximum allowed latency for this application category
    pub fn max_latency_us(&self) -> u64 {
        match self {
            Self::LifeCritical => 100,
            Self::SafetyCritical => 1000,
            Self::RealTimeControl => 10000,
            Self::ComfortSystems => 100000,
            Self::Infotainment => 1000000,
            Self::Diagnostics => 10000000,
        }
    }
    
    /// Get maximum allowed error rate (parts per million)
    pub fn max_error_rate_ppm(&self) -> u64 {
        match self {
            Self::LifeCritical => 0, // Zero tolerance for life-critical
            Self::SafetyCritical => 1,
            Self::RealTimeControl => 10,
            Self::ComfortSystems => 100,
            Self::Infotainment => 1000,
            Self::Diagnostics => 10000,
        }
    }
    
    /// Get required ASIL level
    pub fn required_asil_level(&self) -> AsilLevel {
        match self {
            Self::LifeCritical => AsilLevel::D,
            Self::SafetyCritical => AsilLevel::C,  
            Self::RealTimeControl => AsilLevel::B,
            Self::ComfortSystems => AsilLevel::A,
            Self::Infotainment | Self::Diagnostics => AsilLevel::A, // QM treated as ASIL-A
        }
    }
}

/// Comprehensive automotive metrics for safety-critical evaluation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomotiveMetrics {
    // Timing metrics
    pub deadline_misses: u64,
    pub total_operations: u64,
    pub deadline_miss_rate_ppm: f64,
    pub worst_case_latency_us: u64,
    pub best_case_latency_us: u64,
    pub average_latency_us: f64,
    pub jitter_us: u64, // max - min latency
    
    // Safety metrics
    pub consecutive_failures: u64,
    pub max_consecutive_failures: u64,
    pub error_count: u64,
    pub error_rate_ppm: f64,
    pub safety_violations: Vec<AutomotiveError>,
    
    // ASIL compliance
    pub asil_level: AsilLevel,
    pub asil_compliant: bool,
    pub safety_margin_percent: f64, // How much safety margin remains
    
    // Determinism metrics
    pub determinism_score: f64, // 0.0-1.0 (higher = more deterministic)
    pub timing_variability: f64, // Coefficient of variation
    
    // Test configuration
    pub mechanism: IpcMechanism,
    pub message_size_bytes: usize,
    pub test_duration: Duration,
    pub automotive_application: AutomotiveApplication,
}

impl AutomotiveMetrics {
    /// Create new automotive metrics collector
    pub fn new(
        mechanism: IpcMechanism,
        message_size: usize, 
        asil_level: AsilLevel,
        application: AutomotiveApplication,
    ) -> Self {
        Self {
            deadline_misses: 0,
            total_operations: 0,
            deadline_miss_rate_ppm: 0.0,
            worst_case_latency_us: 0,
            best_case_latency_us: u64::MAX,
            average_latency_us: 0.0,
            jitter_us: 0,
            consecutive_failures: 0,
            max_consecutive_failures: 0,
            error_count: 0,
            error_rate_ppm: 0.0,
            safety_violations: Vec::new(),
            asil_level,
            asil_compliant: true,
            safety_margin_percent: 100.0,
            determinism_score: 0.0,
            timing_variability: 0.0,
            mechanism,
            message_size_bytes: message_size,
            test_duration: Duration::ZERO,
            automotive_application: application,
        }
    }
    
    /// Record a successful message operation
    pub fn record_success(&mut self, latency: Duration) {
        let latency_us = latency.as_micros() as u64;
        
        self.total_operations += 1;
        
        // Update latency statistics
        self.worst_case_latency_us = self.worst_case_latency_us.max(latency_us);
        self.best_case_latency_us = self.best_case_latency_us.min(latency_us);
        
        // Update running average
        let total = self.total_operations as f64;
        self.average_latency_us = (self.average_latency_us * (total - 1.0) + latency_us as f64) / total;
        
        // Update jitter
        if self.best_case_latency_us != u64::MAX {
            self.jitter_us = self.worst_case_latency_us - self.best_case_latency_us;
        }
        
        // Check deadline compliance
        let deadline = self.automotive_application.max_latency_us();
        if latency_us > deadline {
            self.record_deadline_miss(latency_us, deadline);
        } else {
            // Reset consecutive failure count on success
            self.consecutive_failures = 0;
        }
        
        self.update_rates_and_compliance();
    }
    
    /// Record a deadline miss (automotive safety violation)
    pub fn record_deadline_miss(&mut self, latency_us: u64, deadline_us: u64) {
        self.deadline_misses += 1;
        self.error_count += 1;
        self.consecutive_failures += 1;
        
        // Track maximum consecutive failures
        self.max_consecutive_failures = self.max_consecutive_failures.max(self.consecutive_failures);
        
        // Record safety violation
        let error = AutomotiveError::DeadlineMissed {
            latency_us,
            deadline_us,
            mechanism: self.mechanism,
        };
        self.safety_violations.push(error);
        
        // Check if consecutive failures exceed safety threshold
        let max_consecutive = match self.asil_level {
            AsilLevel::D => 0, // Zero tolerance
            AsilLevel::C => 2,
            AsilLevel::B => 5, 
            AsilLevel::A => 10,
        };
        
        if self.consecutive_failures > max_consecutive {
            let error = AutomotiveError::ConsecutiveFailuresExceeded {
                count: self.consecutive_failures,
                max_allowed: max_consecutive,
            };
            self.safety_violations.push(error);
            self.asil_compliant = false;
        }
    }
    
    /// Update error rates and ASIL compliance status
    fn update_rates_and_compliance(&mut self) {
        if self.total_operations > 0 {
            // Calculate error rates
            self.deadline_miss_rate_ppm = (self.deadline_misses as f64 / self.total_operations as f64) * 1_000_000.0;
            self.error_rate_ppm = (self.error_count as f64 / self.total_operations as f64) * 1_000_000.0;
            
            // Check ASIL compliance
            let max_allowed_ppm = self.automotive_application.max_error_rate_ppm();
            self.asil_compliant = self.error_rate_ppm <= max_allowed_ppm as f64;
            
            if !self.asil_compliant {
                let error = AutomotiveError::ErrorBudgetExceeded {
                    error_count_ppm: self.error_rate_ppm as u64,
                    max_allowed_ppm: max_allowed_ppm,
                    asil_level: self.asil_level.clone(),
                };
                self.safety_violations.push(error);
            }
            
            // Calculate safety margin
            if max_allowed_ppm > 0 {
                self.safety_margin_percent = ((max_allowed_ppm as f64 - self.error_rate_ppm) / max_allowed_ppm as f64) * 100.0;
                self.safety_margin_percent = self.safety_margin_percent.max(0.0);
            } else {
                self.safety_margin_percent = if self.error_rate_ppm == 0.0 { 100.0 } else { 0.0 };
            }
        }
    }
    
    /// Calculate determinism score based on timing variability
    pub fn calculate_determinism_score(&mut self, latency_samples: &[Duration]) {
        if latency_samples.len() < 2 {
            return;
        }
        
        // Calculate coefficient of variation (std_dev / mean)
        let mean = self.average_latency_us;
        let variance = latency_samples.iter()
            .map(|d| {
                let latency = d.as_micros() as f64;
                (latency - mean).powi(2)
            })
            .sum::<f64>() / latency_samples.len() as f64;
        
        let std_dev = variance.sqrt();
        self.timing_variability = if mean > 0.0 { std_dev / mean } else { 1.0 };
        
        // Determinism score: lower variability = higher score
        // Perfect determinism (0 variability) = 1.0
        // High variability (>1.0 CV) = approaching 0.0
        self.determinism_score = 1.0 / (1.0 + self.timing_variability);
    }
    
    /// Generate automotive suitability assessment
    pub fn evaluate_automotive_suitability(&self) -> AutomotiveSuitabilityReport {
        let mut suitable_applications = Vec::new();
        let mut issues = Vec::new();
        let mut recommendations = Vec::new();
        
        // Check each application category
        for app in [
            AutomotiveApplication::LifeCritical,
            AutomotiveApplication::SafetyCritical, 
            AutomotiveApplication::RealTimeControl,
            AutomotiveApplication::ComfortSystems,
            AutomotiveApplication::Infotainment,
            AutomotiveApplication::Diagnostics,
        ] {
            let max_latency = app.max_latency_us();
            let max_error_rate = app.max_error_rate_ppm();
            
            if self.worst_case_latency_us <= max_latency && 
               self.error_rate_ppm <= max_error_rate as f64 {
                suitable_applications.push(app);
            }
        }
        
        // Generate issues and recommendations
        if self.deadline_misses > 0 {
            issues.push(format!("Deadline misses detected: {} out of {} operations", self.deadline_misses, self.total_operations));
            recommendations.push("Consider optimizing for lower latency or relaxing timing requirements".to_string());
        }
        
        if !self.asil_compliant {
            issues.push(format!("ASIL-{:?} compliance failed", self.asil_level));
            recommendations.push("Implement additional safety measures or choose different IPC mechanism".to_string());
        }
        
        if self.determinism_score < 0.9 {
            issues.push("Poor timing determinism detected".to_string());
            recommendations.push("Enable real-time scheduling and optimize for consistent timing".to_string());
        }
        
        // Overall suitability score (0-100)
        let latency_score = if self.worst_case_latency_us <= AutomotiveApplication::LifeCritical.max_latency_us() {
            100.0
        } else if self.worst_case_latency_us <= AutomotiveApplication::SafetyCritical.max_latency_us() {
            80.0
        } else if self.worst_case_latency_us <= AutomotiveApplication::RealTimeControl.max_latency_us() {
            60.0
        } else if self.worst_case_latency_us <= AutomotiveApplication::ComfortSystems.max_latency_us() {
            40.0
        } else {
            20.0
        };
        
        let reliability_score = if self.asil_compliant { 100.0 } else { 0.0 };
        let determinism_score = self.determinism_score * 100.0;
        
        let overall_score = (latency_score + reliability_score + determinism_score) / 3.0;
        
        AutomotiveSuitabilityReport {
            mechanism: self.mechanism,
            overall_score,
            suitable_applications,
            max_suitable_asil: self.get_max_suitable_asil(),
            issues,
            recommendations,
            metrics_summary: self.clone(),
        }
    }
    
    fn get_max_suitable_asil(&self) -> AsilLevel {
        if self.worst_case_latency_us <= 100 && self.error_rate_ppm == 0.0 {
            AsilLevel::D
        } else if self.worst_case_latency_us <= 1000 && self.error_rate_ppm <= 1.0 {
            AsilLevel::C
        } else if self.worst_case_latency_us <= 10000 && self.error_rate_ppm <= 10.0 {
            AsilLevel::B
        } else {
            AsilLevel::A
        }
    }
}

/// Comprehensive automotive suitability evaluation report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomotiveSuitabilityReport {
    pub mechanism: IpcMechanism,
    pub overall_score: f64, // 0-100
    pub suitable_applications: Vec<AutomotiveApplication>,
    pub max_suitable_asil: AsilLevel,
    pub issues: Vec<String>,
    pub recommendations: Vec<String>,
    pub metrics_summary: AutomotiveMetrics,
}

/// Automotive deadline enforcement for real-time testing
pub struct AutomotiveDeadlineEnforcer {
    deadline_us: u64,
    asil_level: AsilLevel,
}

impl AutomotiveDeadlineEnforcer {
    pub fn new(deadline_us: u64, asil_level: AsilLevel) -> Self {
        Self {
            deadline_us,
            asil_level,
        }
    }
    
    /// Check if operation meets automotive deadline requirements
    pub fn check_deadline(&self, latency: Duration) -> Result<(), AutomotiveError> {
        let latency_us = latency.as_micros() as u64;
        
        if latency_us > self.deadline_us {
            Err(AutomotiveError::DeadlineMissed {
                latency_us,
                deadline_us: self.deadline_us,
                mechanism: IpcMechanism::UnixDomainSocket, // Will be set by caller
            })
        } else {
            Ok(())
        }
    }
    
    /// Get automotive-appropriate timeout for operations
    pub fn get_timeout_duration(&self) -> Duration {
        // Use deadline as timeout, but cap based on ASIL level
        let timeout_us = match self.asil_level {
            AsilLevel::D => self.deadline_us.min(100), // Life-critical: very short timeout
            AsilLevel::C => self.deadline_us.min(1000),
            AsilLevel::B => self.deadline_us.min(10000),
            AsilLevel::A => self.deadline_us.min(100000),
        };
        
        Duration::from_micros(timeout_us)
    }
}
