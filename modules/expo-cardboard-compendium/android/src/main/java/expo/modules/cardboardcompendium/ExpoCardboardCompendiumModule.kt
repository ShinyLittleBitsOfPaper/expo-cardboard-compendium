package expo.modules.cardboardcompendium

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

class ExpoCardboardCompendiumModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("ExpoCardboardCompendium")
    // AsyncFunction runs off the JS thread; the engine serializes calls internally.
    AsyncFunction("call") { method: String, args: String -> Native.call(method, args) }
  }
}
