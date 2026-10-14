#pragma once
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
int32_t spatial_mobile_start(const char *profile, const char *email, const char *password, const char *identity_path);
char *spatial_mobile_poll(void);
void spatial_mobile_stop(void);
int32_t spatial_mobile_local_map(const char *sample);
void spatial_mobile_pose(const char *sample);
void spatial_mobile_depth(const char *sample);
int32_t spatial_mobile_annotate(const char *request);
int32_t spatial_mobile_survey(const char *sample);
void spatial_mobile_string_free(char *value);
#ifdef __cplusplus
}
#endif
