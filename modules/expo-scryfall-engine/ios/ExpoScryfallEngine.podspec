Pod::Spec.new do |s|
  s.name           = 'ExpoScryfallEngine'
  s.version        = '0.0.1'
  s.summary        = 'Scryfall-syntax card search over COQD/COQS (Rust)'
  s.homepage       = 'https://example.invalid/expo-scryfall-engine'
  s.license        = { :type => 'MIT' }
  s.author         = 'slbop'
  s.platforms      = { :ios => '15.1' }
  s.source         = { :git => '' }
  s.static_framework = true
  s.dependency 'ExpoModulesCore'
  s.source_files   = '*.{h,swift}'
  # XCFramework with libcoqe.a per slice, produced by scripts/build-native.sh
  s.vendored_frameworks = 'libcoqe.xcframework'
end
