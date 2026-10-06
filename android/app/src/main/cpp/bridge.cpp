#include <jni.h>
#include <algorithm>
#include <string>
#include "SpatialPortalBridge.h"

// Kotlin passes UTF-8 bytes, not JNI modified UTF-8. Copy before calling Rust.
static std::string utf8(JNIEnv* env, jbyteArray value) {
    if (!value) return {};
    const auto length = env->GetArrayLength(value);
    std::string result(length, '\0');
    env->GetByteArrayRegion(value, 0, length, reinterpret_cast<jbyte*>(result.data()));
    return result;
}
extern "C" JNIEXPORT jboolean JNICALL
Java_com_aukilabs_annotator_NativeBridge_start(JNIEnv* env, jobject, jbyteArray profile,
    jbyteArray email, jbyteArray password, jbyteArray identity) {
    auto p = utf8(env, profile), e = utf8(env, email), secret = utf8(env, password), i = utf8(env, identity);
    if (env->ExceptionCheck() || p.find('\0') != std::string::npos || e.find('\0') != std::string::npos ||
        secret.find('\0') != std::string::npos || i.find('\0') != std::string::npos) {
        std::fill(secret.begin(), secret.end(), '\0');
        return false;
    }
    const auto result = spatial_mobile_start(p.c_str(), e.c_str(), secret.c_str(), i.c_str());
    std::fill(secret.begin(), secret.end(), '\0');
    return result != 0;
}
extern "C" JNIEXPORT jbyteArray JNICALL
Java_com_aukilabs_annotator_NativeBridge_poll(JNIEnv* env, jobject) {
    char* raw = spatial_mobile_poll();
    if (!raw) return nullptr;
    std::string snapshot(raw);
    spatial_mobile_string_free(raw);
    auto result = env->NewByteArray(static_cast<jsize>(snapshot.size()));
    if (result) env->SetByteArrayRegion(result, 0, static_cast<jsize>(snapshot.size()),
        reinterpret_cast<const jbyte*>(snapshot.data()));
    return result;
}
extern "C" JNIEXPORT void JNICALL
Java_com_aukilabs_annotator_NativeBridge_stop(JNIEnv*, jobject) { spatial_mobile_stop(); }
extern "C" JNIEXPORT jboolean JNICALL
Java_com_aukilabs_annotator_NativeBridge_localMap(JNIEnv* env, jobject, jbyteArray value) {
    if (!value) return spatial_mobile_local_map(nullptr) != 0;
    if (env->GetArrayLength(value) > 512 * 1024) return false;
    auto sample = utf8(env, value);
    if (env->ExceptionCheck() || sample.find('\0') != std::string::npos) return false;
    return spatial_mobile_local_map(sample.c_str()) != 0;
}
