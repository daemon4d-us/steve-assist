terraform {
  required_version = ">= 1.5"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.0"
    }
  }
}

provider "google" {
  project = var.project_id
  region  = var.region
}

# Artifact Registry repository for Docker images
resource "google_artifact_registry_repository" "steve_assist" {
  location      = var.region
  repository_id = "steve-assist"
  format        = "DOCKER"
  description   = "Docker repository for steve-assist"
}

# GKE cluster
resource "google_container_cluster" "primary" {
  name     = var.cluster_name
  location = var.zone

  # Use a separately managed node pool
  remove_default_node_pool = true
  initial_node_count       = 1

  # Enable Workload Identity
  workload_identity_config {
    workload_pool = "${var.project_id}.svc.id.goog"
  }

  deletion_protection = false
}

# Node pool
resource "google_container_node_pool" "primary_nodes" {
  name     = "primary-pool"
  location = var.zone
  cluster  = google_container_cluster.primary.name

  autoscaling {
    min_node_count = var.min_node_count
    max_node_count = var.max_node_count
  }

  node_config {
    machine_type = var.node_machine_type
    disk_size_gb = 50

    oauth_scopes = [
      "https://www.googleapis.com/auth/cloud-platform",
    ]

    workload_metadata_config {
      mode = "GKE_METADATA"
    }
  }
}

# GCP service account for Workload Identity (Firestore + Speech-to-Text access)
resource "google_service_account" "steve_assist" {
  account_id   = "steve-assist"
  display_name = "Steve Assist Workload Identity SA"
}

resource "google_project_iam_member" "firestore" {
  project = var.project_id
  role    = "roles/datastore.user"
  member  = "serviceAccount:${google_service_account.steve_assist.email}"
}

resource "google_project_iam_member" "speech" {
  project = var.project_id
  role    = "roles/speech.client"
  member  = "serviceAccount:${google_service_account.steve_assist.email}"
}

# Bind K8s service account to GCP service account
# Depends on the node pool to ensure the Workload Identity pool is fully provisioned
resource "google_service_account_iam_member" "workload_identity" {
  service_account_id = google_service_account.steve_assist.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.project_id}.svc.id.goog[steve-assist/steve-assist]"

  depends_on = [google_container_node_pool.primary_nodes]
}

# Static IP for the ingress
resource "google_compute_global_address" "steve_assist" {
  name = "steve-assist-ip"
}

# --- cert-manager DNS01 via Cloud DNS ---

resource "google_service_account" "cert_manager" {
  account_id   = "cert-manager"
  display_name = "cert-manager DNS01 solver"
}

resource "google_project_iam_member" "cert_manager_dns" {
  project = var.dns_project_id
  role    = "roles/dns.admin"
  member  = "serviceAccount:${google_service_account.cert_manager.email}"
}

resource "google_service_account_iam_member" "cert_manager_workload_identity" {
  service_account_id = google_service_account.cert_manager.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${var.project_id}.svc.id.goog[cert-manager/cert-manager]"

  depends_on = [google_container_node_pool.primary_nodes]
}

# --- DNS record pointing to Istio ingress gateway ---

resource "google_dns_record_set" "steve_assist" {
  project      = var.dns_project_id
  name         = "${var.domain}."
  type         = "A"
  ttl          = 300
  managed_zone = var.dns_zone_name

  # This will be updated after Istio ingress gateway gets its external IP.
  # For now, use the static IP. You can also set this manually.
  rrdatas = [google_compute_global_address.steve_assist.address]
}

resource "google_dns_record_set" "dashboard" {
  project      = var.dns_project_id
  name         = "dashboard.${var.domain}."
  type         = "A"
  ttl          = 300
  managed_zone = var.dns_zone_name
  rrdatas      = [google_compute_global_address.steve_assist.address]
}
