#pragma once
#include <stdint.h>
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
int32_t spatial_mobile_start(const char *profile, const char *email, const char *password, const char *identity_path);
char *spatial_mobile_poll(void);
void spatial_mobile_stop(void);
int32_t spatial_mobile_local_map(const char *sample);
void spatial_mobile_pose(const char *sample);
void spatial_mobile_sleep(int32_t asleep);
void spatial_mobile_drive(const char *target, float forward, float turn);
int32_t spatial_mobile_report_portal(const char *report);
void spatial_mobile_depth(const char *sample);
int32_t spatial_mobile_camera_demand(int32_t active);
void spatial_mobile_camera(uint64_t timestamp, const uint8_t *jpeg, size_t length);
char *spatial_mobile_calibrate(const uint8_t *luma, size_t length, const char *capture);
int32_t spatial_mobile_annotate(const char *request);
int32_t spatial_mobile_change_note(const char *request);
void spatial_mobile_string_free(char *value);
#ifdef __cplusplus
}
#endif
