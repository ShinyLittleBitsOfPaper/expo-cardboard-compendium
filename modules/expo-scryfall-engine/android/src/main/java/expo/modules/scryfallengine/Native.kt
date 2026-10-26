package expo.modules.scryfallengine

object Native {
  init {
    System.loadLibrary("coqe")
    System.loadLibrary("coqe_jni")
  }
  @JvmStatic external fun call(method: String, args: String): String
}
