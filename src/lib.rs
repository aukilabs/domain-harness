pub mod peer_traffic;
pub mod traffic_history;
pub mod conventions;
pub mod qr_decode;
pub mod compact_pose;
pub mod product_registry;
pub mod frame_alignment;
pub mod origin_reset;
pub mod scene_wifi;
pub mod video_presentation;
pub mod compact_ar;
pub mod declared_depth;
pub mod home_pose_policy;
pub mod catalog_chat;
pub mod commands;
pub mod observation_history;
pub mod capture_time;
pub mod clock_sync;
pub mod time_history;
pub mod scene_geometry;
pub mod peer_aliases;
pub mod domain_portals;
pub mod fisheye;
pub mod local_mapper;
pub mod mobile;
pub mod mobile_home;
pub mod mobile_camera;
pub mod mobile_screen;
pub mod model;
pub mod network;
pub mod peer;
pub mod portal_sizes;
pub mod raycast;
pub mod rgbd;
pub mod runner_pose;
pub mod robot_presence;
pub mod robot_state;
pub mod robot_states;
pub mod registry_robots;
pub mod runner_voxels;
pub mod scene_mirror;
pub mod shared_inputs;
pub mod synthetic_room;
pub mod voxel_color;

pub mod sensor_preview;

pub mod runner_sensors;
pub mod portal_camera_detector;

pub mod home_alignment;
pub mod frame_transform;

pub mod recent_voxels;

pub mod fresh_ingress;

pub mod home_mapper;

pub mod collaborative_mapper;
pub mod scene_contribution;

pub mod ar_preview;
pub mod discovered_ar;
pub mod registry_ar;
pub mod transform_chain;
pub mod depth_points;
pub mod component_enabled;
pub mod camera_frame;
pub mod component_control;
pub mod robot_occupancy;

pub mod notes;

pub mod picking;

pub mod mobile_notes;
pub mod mobile_odometry;

pub mod mobile_portal;
/// First-observed portal origin and explicit mapping reset contracts.
pub mod scene_origin;
pub mod portal_mount;
pub mod mobile_pose;

pub mod mobile_geo;
pub mod mobile_geometry;

pub mod mobile_depth;
pub mod mobile_depth_image;
pub mod drive_contract;
pub mod mobile_drive;
pub mod mobile_calibration;
pub mod onboarding;
pub mod square_pose;
pub mod voxel_delta;
pub mod mobile_onboarding;

pub mod fiducials;
pub mod landmarks;
pub mod occupancy_mapper;
pub mod voxel_layers;
pub mod occupancy_fixture;
pub mod simulated_robot;
pub mod mobile_declared;
pub mod mobile_local;
pub mod mobile_products;
pub mod device_catalogs;
pub mod simulated_camera;
pub mod simulated_lidar;
pub mod simulator_mapper;
pub mod environments;
pub mod local_scenes;
pub mod local_mobile;
pub mod semantic_volumes;
pub mod scene_paths;
pub mod scene_routes;
pub mod scene_navigation;

pub mod robot_runner;
pub mod navigate;
pub mod robot_navigation;
pub mod robot_pointing;
pub mod point_at;
pub mod chatbot;
pub mod view_camera; // Browser-local viewpoint controls; sensor attachments never move hardware.
pub mod camera_vision;
pub mod scene_tools;
pub mod peer_locations;
pub mod chat_camera;
