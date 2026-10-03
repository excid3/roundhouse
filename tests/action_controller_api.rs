//! A controller under `ActionController::API` has the controller
//! surface. (`tests/emit_and_run.rs` runs one.)

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, DiagnosticKind};
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn an_api_controller_has_request_and_params() {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/api/pings_controller.rb",
            "class Api::PingsController < ActionController::API\n  def show\n    render json: {ip: request.remote_ip, id: params[:id].to_s}\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/api/ping\", to: \"api/pings#show\"\nend\n",
        ),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let unknown: Vec<String> = diagnose(&app)
        .into_iter()
        .filter(|d| {
            matches!(
                d.kind,
                DiagnosticKind::UnresolvedType { .. }
                    | DiagnosticKind::GradualUntyped { .. }
                    | DiagnosticKind::SendDispatchFailed { .. }
            )
        })
        .map(|d| d.message)
        .collect();
    assert!(
        unknown.is_empty(),
        "a controller under ActionController::API must have the controller surface:\n{}",
        unknown.join("\n")
    );
}
