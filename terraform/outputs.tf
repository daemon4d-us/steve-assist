output "cluster_name" {
  value = google_container_cluster.primary.name
}

output "cluster_endpoint" {
  value     = google_container_cluster.primary.endpoint
  sensitive = true
}

output "artifact_registry" {
  value = "${var.region}-docker.pkg.dev/${var.project_id}/${google_artifact_registry_repository.steve_assist.repository_id}"
}

output "workload_identity_sa" {
  value = google_service_account.steve_assist.email
}

output "static_ip" {
  value = google_compute_global_address.steve_assist.address
}

output "cert_manager_sa" {
  value = google_service_account.cert_manager.email
}
