import ExpoModulesCore

public class ExpoScryfallEngineModule: Module {
  public func definition() -> ModuleDefinition {
    Name("ExpoScryfallEngine")
    AsyncFunction("call") { (method: String, args: String) -> String in
      guard let r = coqe_call(method, args) else { return "{\"error\":\"engine returned null\"}" }
      defer { coqe_free(r) }
      return String(cString: r)
    }
  }
}
