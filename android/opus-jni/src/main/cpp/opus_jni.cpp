#include <jni.h>
#include <android/log.h>
#include "opus/include/opus.h"

#define LOG_TAG "OpusJNI"
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, LOG_TAG, __VA_ARGS__)

extern "C" {

// Creates an Opus encoder and returns its handle as a jlong pointer.
// Returns 0 on failure.
JNIEXPORT jlong JNICALL
Java_com_anymic_opus_OpusNative_createEncoder(JNIEnv* /*env*/, jclass /*clazz*/,
                                              jint sampleRate, jint channels, jint application) {
    int error = OPUS_OK;
    OpusEncoder* encoder = opus_encoder_create(sampleRate, channels, application, &error);
    if (error != OPUS_OK || encoder == nullptr) {
        LOGE("opus_encoder_create failed: %s", opus_strerror(error));
        return 0L;
    }
    return reinterpret_cast<jlong>(encoder);
}

// Destroys the Opus encoder referenced by handle.
JNIEXPORT void JNICALL
Java_com_anymic_opus_OpusNative_destroyEncoder(JNIEnv* /*env*/, jclass /*clazz*/, jlong handle) {
    if (handle == 0L) return;
    OpusEncoder* encoder = reinterpret_cast<OpusEncoder*>(handle);
    opus_encoder_destroy(encoder);
}

// Sets the bitrate of the encoder. Returns OPUS_OK (0) on success, negative on error.
JNIEXPORT jint JNICALL
Java_com_anymic_opus_OpusNative_setBitrate(JNIEnv* /*env*/, jclass /*clazz*/,
                                           jlong handle, jint bitrate) {
    if (handle == 0L) return OPUS_BAD_ARG;
    OpusEncoder* encoder = reinterpret_cast<OpusEncoder*>(handle);
    return opus_encoder_ctl(encoder, OPUS_SET_BITRATE(bitrate));
}

// Encodes PCM samples. Returns number of bytes written to outBuf, or negative Opus error code.
JNIEXPORT jint JNICALL
Java_com_anymic_opus_OpusNative_encode(JNIEnv* env, jclass /*clazz*/,
                                       jlong handle,
                                       jshortArray pcmIn, jint pcmSamplesPerChannel,
                                       jbyteArray outBuf, jint outBufMaxLen) {
    if (handle == 0L) return OPUS_BAD_ARG;
    OpusEncoder* encoder = reinterpret_cast<OpusEncoder*>(handle);

    jshort* pcm = env->GetShortArrayElements(pcmIn, nullptr);
    if (pcm == nullptr) return OPUS_BAD_ARG;

    jbyte* out = env->GetByteArrayElements(outBuf, nullptr);
    if (out == nullptr) {
        env->ReleaseShortArrayElements(pcmIn, pcm, JNI_ABORT);
        return OPUS_BAD_ARG;
    }

    opus_int32 result = opus_encode(encoder,
                                    reinterpret_cast<const opus_int16*>(pcm),
                                    pcmSamplesPerChannel,
                                    reinterpret_cast<unsigned char*>(out),
                                    static_cast<opus_int32>(outBufMaxLen));

    env->ReleaseShortArrayElements(pcmIn, pcm, JNI_ABORT);
    env->ReleaseByteArrayElements(outBuf, out, result > 0 ? 0 : JNI_ABORT);

    return static_cast<jint>(result);
}

// Returns the libopus version string.
JNIEXPORT jstring JNICALL
Java_com_anymic_opus_OpusNative_libopusVersion(JNIEnv* env, jclass /*clazz*/) {
    return env->NewStringUTF(opus_get_version_string());
}

} // extern "C"
