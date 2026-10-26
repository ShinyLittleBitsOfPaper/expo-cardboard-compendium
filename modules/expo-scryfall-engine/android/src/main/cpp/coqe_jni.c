#include <jni.h>
#include <stdlib.h>

extern char *coqe_call(const char *method, const char *args);
extern void coqe_free(char *p);

JNIEXPORT jstring JNICALL Java_expo_modules_scryfallengine_Native_call(JNIEnv *env, jclass cls, jstring method, jstring args) {
  const char *m = (*env)->GetStringUTFChars(env, method, NULL);
  const char *a = (*env)->GetStringUTFChars(env, args, NULL);
  char *r = coqe_call(m, a);
  (*env)->ReleaseStringUTFChars(env, method, m);
  (*env)->ReleaseStringUTFChars(env, args, a);
  jstring out = (*env)->NewStringUTF(env, r);
  coqe_free(r);
  return out;
}
