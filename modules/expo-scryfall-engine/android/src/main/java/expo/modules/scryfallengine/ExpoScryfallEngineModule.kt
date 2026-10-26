package expo.modules.scryfallengine

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

class ExpoScryfallEngineModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("ExpoScryfallEngine")
    // AsyncFunction runs off the JS thread; the engine serializes calls internally.
    AsyncFunction("call") { method: String, args: String -> Native.call(method, args) }
  }
}
