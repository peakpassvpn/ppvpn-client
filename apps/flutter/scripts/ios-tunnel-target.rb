# The packet tunnel extension (#448) added to the iOS runner flutter create
# made (scripts/ci-build.sh, after its own edits of the project): target
# PacketTunnel, bundle id com.peakpassvpn.ppvpn.tunnel, from runner/ios
# PacketTunnel (copied to ios/PacketTunnel), embedded in the app. Also the
# privacy manifests of both targets.
#
#   ruby ios-tunnel-target.rb <project> <version> <build>
#
# Environment, all optional:
#   IOS_TEAM_ID, IOS_TUNNEL_PROFILE   App Store signing, as the Runner's
#   PPVPN_TUNNEL_XCFRAMEWORK          the tunnel host (crates/tunnel-host,
#                                     PPVPNTunnel.xcframework), linked into
#                                     the extension only; without it the
#                                     extension refuses to start
#                                     (TunnelHost.swift)
#   PPVPN_TUNNEL_LIBS                 the system libraries the host needs,
#                                     as linker flags
require "xcodeproj"

project_path, version, build = ARGV
abort "usage: ios-tunnel-target.rb <project> <version> <build>" unless build

project = Xcodeproj::Project.open(project_path)
runner = project.targets.find { |target| target.name == "Runner" } or abort "#{project_path}: no Runner target"
abort "#{project_path}: PacketTunnel exists already" if project.targets.any? { |target| target.name == "PacketTunnel" }

tunnel = project.new_target(:app_extension, "PacketTunnel", :ios, "15.0", nil, :swift)
# Every configuration the project has (flutter's Debug, Release, Profile).
project.build_configurations.each do |config|
  next if tunnel.build_configurations.any? { |own| own.name == config.name }
  tunnel.add_build_configuration(config.name, config.name == "Debug" ? :debug : :release)
end

group = project.main_group.find_subpath("PacketTunnel", true)
group.set_source_tree("<group>")
group.set_path("PacketTunnel")
sources = %w[PacketTunnelProvider.swift TunnelHost.swift].map { |name| group.new_reference(name) }
tunnel.add_file_references(sources)
%w[Info.plist PacketTunnel.entitlements].each { |name| group.new_reference(name) }
tunnel.add_system_framework("NetworkExtension")

# The privacy manifests (#535), from runner/ios: each target's in its own
# bundle.
tunnel.resources_build_phase.add_file_reference(group.new_reference("PrivacyInfo.xcprivacy"), true)
runner_group = project.main_group.find_subpath("Runner", false) or abort "#{project_path}: no Runner group"
abort "Runner/PrivacyInfo.xcprivacy is missing" unless File.file?(File.join(File.dirname(project_path), "Runner", "PrivacyInfo.xcprivacy"))
runner.resources_build_phase.add_file_reference(runner_group.new_reference("PrivacyInfo.xcprivacy"), true)

xcframework = ENV["PPVPN_TUNNEL_XCFRAMEWORK"].to_s
unless xcframework.empty?
  abort "#{xcframework}: no such XCFramework" unless File.directory?(xcframework)
  reference = project.frameworks_group.new_reference(File.expand_path(xcframework))
  tunnel.frameworks_build_phase.add_file_reference(reference, true)
end

team = ENV["IOS_TEAM_ID"].to_s
tunnel.build_configurations.each do |config|
  settings = config.build_settings
  settings["PRODUCT_BUNDLE_IDENTIFIER"] = "com.peakpassvpn.ppvpn.tunnel"
  settings["PRODUCT_NAME"] = "$(TARGET_NAME)"
  settings["INFOPLIST_FILE"] = "PacketTunnel/Info.plist"
  settings["MARKETING_VERSION"] = version
  settings["CURRENT_PROJECT_VERSION"] = build
  settings["IPHONEOS_DEPLOYMENT_TARGET"] = "15.0"
  settings["TARGETED_DEVICE_FAMILY"] = "1,2"
  settings["SWIFT_VERSION"] = "5.0"
  settings["SKIP_INSTALL"] = "YES"
  settings["APPLICATION_EXTENSION_API_ONLY"] = "YES"
  # The system limits the extension to 50 MiB (#388): no debug dylib.
  settings["ENABLE_DEBUG_DYLIB"] = "NO"
  # Apple silicon's simulator only: the XCFramework's simulator slice is
  # arm64, and a simulator build for x86_64 too would link without it
  # (the Runner's architectures come from flutter's xcconfig, which this
  # target does not have).
  settings["EXCLUDED_ARCHS[sdk=iphonesimulator*]"] = "i386 x86_64"
  libs = ENV["PPVPN_TUNNEL_LIBS"].to_s.split
  settings["OTHER_LDFLAGS"] = ["$(inherited)", *libs] unless libs.empty?
  next if team.empty?
  profile = ENV["IOS_TUNNEL_PROFILE"].to_s
  abort "IOS_TUNNEL_PROFILE is required with IOS_TEAM_ID" if profile.empty?
  settings["CODE_SIGN_STYLE"] = "Manual"
  settings["DEVELOPMENT_TEAM"] = team
  settings["CODE_SIGN_IDENTITY"] = "Apple Distribution"
  settings["CODE_SIGN_IDENTITY[sdk=iphoneos*]"] = "Apple Distribution"
  settings["PROVISIONING_PROFILE_SPECIFIER"] = profile
  settings["CODE_SIGN_ENTITLEMENTS"] = "PacketTunnel/PacketTunnel.entitlements"
end

# Into the app's PlugIns, before flutter's Thin Binary script: after it,
# Xcode reports a dependency cycle.
embed = runner.new_copy_files_build_phase("Embed Foundation Extensions")
embed.dst_subfolder_spec = Xcodeproj::Constants::COPY_FILES_BUILD_PHASE_DESTINATIONS[:plug_ins]
embed.add_file_reference(tunnel.product_reference, true).settings = { "ATTRIBUTES" => ["RemoveHeadersOnCopy"] }
runner.build_phases.delete(embed)
thin = runner.build_phases.index { |phase| phase.respond_to?(:name) && phase.name == "Thin Binary" }
runner.build_phases.insert(thin || runner.build_phases.size, embed)
runner.add_dependency(tunnel)

project.save
puts "PacketTunnel: #{tunnel.build_configurations.map(&:name).join(', ')}" \
     "#{xcframework.empty? ? ', no tunnel host' : ", host #{File.basename(xcframework)}"}" \
     "#{team.empty? ? '' : ', signed'}"
