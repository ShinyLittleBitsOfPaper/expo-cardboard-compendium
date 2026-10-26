import ExpoModulesCore

public class ExpoCardboardCompendiumModule: Module {
  public func definition() -> ModuleDefinition {
    Name("ExpoCardboardCompendium")
    AsyncFunction("call") { (method: String, args: String) -> String in
      guard let r = coqe_call(method, args) else { return "{\"error\":\"engine returned null\"}" }
      defer { coqe_free(r) }
      return String(cString: r)
    }
  }
}
