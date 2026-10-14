#pragma once
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
int32_t spatial_mobile_start(const char *profile, const char *email, const char *password, const char *identity_path);
int32_t spatial_mobile_start_app(const char *profile, const char *key, const char *secret, const char *identity_path);
char *spatial_mobile_poll(void);
void spatial_mobile_stop(void);
int32_t spatial_mobile_local_map(const char *sample);
void spatial_mobile_pose(const char *sample);
void spatial_mobile_configure_camera(const char *configuration);
void spatial_mobile_sleep(int32_t asleep);
void spatial_mobile_name(const char *name);
void spatial_mobile_geo(const char *reference);
void spatial_mobile_drive(const char *target, float forward, float turn);
int32_t spatial_mobile_report_portal(const char *report);
int32_t spatial_mobile_change_path(const char *change);
int32_t spatial_mobile_ask_route(const char *asked);
int32_t spatial_mobile_report_origin(const char *report);
void spatial_mobile_depth(const char *sample);
void spatial_mobile_placement(const char *placement);
int32_t spatial_mobile_camera_demand(int32_t active);
void spatial_mobile_camera(uint64_t timestamp, const uint8_t *jpeg, size_t length);
void spatial_mobile_camera_calibrated(uint64_t timestamp, const uint8_t *jpeg, size_t length, const char *session, double fx, double fy, double cx, double cy, int32_t rotation_deg, uint64_t alignment_timestamp_ns);
void spatial_mobile_depth_image(uint64_t timestamp, const char *session, uint32_t width, uint32_t height, const uint16_t *depth_mm, const uint8_t *confidence, double fx, double fy, double cx, double cy, uint64_t alignment_timestamp_ns);
char *spatial_mobile_calibrate(const uint8_t *luma, size_t length, const char *capture);
int32_t spatial_mobile_annotate(const char *request);
int32_t spatial_mobile_change_note(const char *request);
void spatial_mobile_string_free(char *value);
#ifdef __cplusplus
}
#endif
