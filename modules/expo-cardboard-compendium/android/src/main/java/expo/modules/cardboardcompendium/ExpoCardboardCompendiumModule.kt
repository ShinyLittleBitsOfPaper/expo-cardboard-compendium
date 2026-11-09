package expo.modules.cardboardcompendium

import expo.modules.kotlin.Promise
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.util.concurrent.Executors

class ExpoCardboardCompendiumModule : Module() {
  // A plain AsyncFunction runs on the module's single queue, so ONE long call (an 80 s dataset patch in the slbop 0.46.0 log) made every
  // other call wait behind it, including the stateless file helpers that take no engine lock. Calls run on their own threads instead;
  // the engine still serialises the calls that need it behind its own lock (see ffi.rs).
  private val pool = Executors.newCachedThreadPool()

  override fun definition() = ModuleDefinition {
    Name("ExpoCardboardCompendium")
    AsyncFunction("call") { method: String, args: String, promise: Promise ->
      pool.execute {
        try {
          promise.resolve(Native.call(method, args))
        } catch (e: Throwable) {
          promise.reject("ERR_ENGINE_CALL", e.message ?: "engine call failed", e)
        }
      }
    }
  }
}
