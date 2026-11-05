// System navigation chrome hosted by Flutter. No client or tunnel state lives here.
import Flutter
import UIKit
import CoreText

enum PPVPNNativeChrome {
  static var available: Bool {
    if #available(iOS 26.0, *) { return true }
    return false
  }

  static func register(with registrar: FlutterPluginRegistrar) {
    if #available(iOS 26.0, *) {
      for name in ["MaterialSymbols", "MaterialSymbolsFilled"] {
        let key = registrar.lookupKey(forAsset: "assets/fonts/\(name).ttf")
        let urls = [Bundle.main.url(forResource: key, withExtension: nil),
          Bundle.main.privateFrameworksURL?.appendingPathComponent("App.framework/flutter_assets/assets/fonts/\(name).ttf")]
        if let url = urls.compactMap({ $0 }).first(where: { FileManager.default.fileExists(atPath: $0.path) }) {
          CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
        }
      }
      MainActor.assumeIsolated {
        for kind in ["navigation", "tabs"] {
          // Hit-test blocking preserves overlays without delaying UIKit selection gestures.
          // See flutter/flutter#191267 for the iOS 26 gesture-arena failure.
          registrar.register(ChromeFactory(kind: kind, messenger: registrar.messenger()),
            withId: "com.peakpassvpn.ppvpn/native-\(kind)",
            gestureRecognizersBlockingPolicy: FlutterPlatformViewGestureRecognizersBlockingPolicyDoNotBlockGesture)
        }
      }
    }
  }
}

@MainActor
private func materialImage(_ code: UInt32, filled: Bool = false) -> UIImage? {
  guard let scalar = UnicodeScalar(code),
    let font = UIFont(name: filled ? "PPVPNMaterialFilled" : "PPVPNMaterial", size: 24) else { return nil }
  return UIGraphicsImageRenderer(size: CGSize(width: 24, height: 24)).image { _ in
    let text = String(scalar) as NSString
    let attributes: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: UIColor.black]
    let size = text.size(withAttributes: attributes)
    text.draw(at: CGPoint(x: (24-size.width)/2, y: (24-size.height)/2), withAttributes: attributes)
  }.withRenderingMode(.alwaysTemplate)
}

@MainActor
private final class ChromeFactory: NSObject, @preconcurrency FlutterPlatformViewFactory {
  let kind: String
  let messenger: FlutterBinaryMessenger
  init(kind: String, messenger: FlutterBinaryMessenger) {
    self.kind = kind
    self.messenger = messenger
    super.init()
  }
  func createArgsCodec() -> FlutterMessageCodec & NSObjectProtocol {
    FlutterStandardMessageCodec.sharedInstance()
  }
  func create(withFrame frame: CGRect, viewIdentifier id: Int64, arguments args: Any?) -> FlutterPlatformView {
    if #available(iOS 26.0, *) {
      return ChromePlatformView(frame: frame, id: id, kind: kind,
        config: args as? [String: Any] ?? [:], messenger: messenger)
    }
    return EmptyChromeView()
  }
}

@MainActor
private final class EmptyChromeView: NSObject, @preconcurrency FlutterPlatformView {
  private let empty = UIView()
  func view() -> UIView { empty }
}

@available(iOS 26.0, *)
@MainActor
private final class ChromePlatformView: NSObject, @preconcurrency FlutterPlatformView, UITabBarControllerDelegate, UINavigationBarDelegate {
  let surface: ChromeSurface
  let channel: FlutterMethodChannel
  private var config: [String: Any] = [:]
  private var applied = false

  init(frame: CGRect, id: Int64, kind: String, config: [String: Any], messenger: FlutterBinaryMessenger) {
    surface = ChromeSurface(frame: frame, kind: kind)
    channel = FlutterMethodChannel(name: "com.peakpassvpn.ppvpn/native-chrome/\(id)", binaryMessenger: messenger)
    super.init()
    surface.navigation.delegate = self
    surface.tabController.delegate = self
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      if call.method == "snapshot" {
        self.snapshot(result, deadline: CACurrentMediaTime() + 1)
        return
      }
      guard call.method == "update", let next = call.arguments as? [String: Any] else {
        return result(FlutterMethodNotImplemented)
      }
      self.apply(next)
      result(nil)
    }
    apply(config)
  }

  deinit {
    channel.setMethodCallHandler(nil)
    // Temporary Flutter offstage changes are not controller destruction.
    let controller = surface.tabController
    Task { @MainActor in
      if controller.parent != nil {
        controller.willMove(toParent: nil)
        controller.removeFromParent()
      }
    }
  }
  func view() -> UIView { surface }

  // Only cache a settled tab indicator. A screenshot of the selection spring
  // would replay its halfway position when the covered page becomes visible.
  private func snapshot(_ result: @escaping FlutterResult, deadline: CFTimeInterval, stableFrames: Int = 0) {
    surface.layoutIfNeeded()
    guard surface.window != nil, surface.bounds.width > 0, surface.bounds.height > 0 else { return result(nil) }
    if surface.kind == "tabs" {
      let moving = Self.geometryMoving(surface.layer)
      let stable = moving ? 0 : stableFrames + 1
      if stable < 2 {
        guard CACurrentMediaTime() < deadline else { return result(nil) }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0 / 60.0) { [weak self] in
          guard let self else { return result(nil) }
          self.snapshot(result, deadline: deadline, stableFrames: stable)
        }
        return
      }
    }
    let format = UIGraphicsImageRendererFormat()
    format.opaque = false
    let image = UIGraphicsImageRenderer(bounds: surface.bounds, format: format).image { _ in
      surface.drawHierarchy(in: surface.bounds, afterScreenUpdates: true)
    }
    result(image.pngData().map { FlutterStandardTypedData(bytes: $0) })
  }

  private static func geometryMoving(_ layer: CALayer) -> Bool {
    if let visible = layer.presentation() {
      if abs(visible.position.x - layer.position.x) > 0.25 || abs(visible.position.y - layer.position.y) > 0.25 ||
        abs(visible.bounds.width - layer.bounds.width) > 0.25 || abs(visible.bounds.height - layer.bounds.height) > 0.25 ||
        abs(visible.transform.m41 - layer.transform.m41) > 0.25 || abs(visible.transform.m42 - layer.transform.m42) > 0.25 ||
        abs(visible.transform.m11 - layer.transform.m11) > 0.001 || abs(visible.transform.m22 - layer.transform.m22) > 0.001 {
        return true
      }
    }
    return (layer.sublayers ?? []).contains(where: geometryMoving)
  }

  private func emit(_ action: String) { channel.invokeMethod("action", arguments: action) }

  private func apply(_ next: [String: Any]) {
    if applied && NSDictionary(dictionary: config).isEqual(to: next) { return }
    applied = true
    config = next
    surface.overrideUserInterfaceStyle = next["dark"] as? Bool == true ? .dark : .light
    let color = (next["accent"] as? NSNumber)?.uint32Value ?? 0xFF3D63C9
    let accent = UIColor(red: CGFloat((color >> 16) & 255) / 255,
      green: CGFloat((color >> 8) & 255) / 255, blue: CGFloat(color & 255) / 255, alpha: 1)
    surface.navigation.tintColor = accent
    surface.tabs.tintColor = accent
    surface.topInset = CGFloat((next["topInset"] as? NSNumber)?.doubleValue ?? 0)
    if surface.kind == "tabs" {
      let labels = next["labels"] as? [String] ?? []
      let symbols: [UInt32] = [57381, 57361, 57386]
      let names = ["overview", "nodes", "me"]
      let items = labels.prefix(3).enumerated().map { index, label in
        let item = UITabBarItem(title: label, image: materialImage(symbols[index]), tag: index)
        item.selectedImage = materialImage(symbols[index], filled: true)
        item.accessibilityIdentifier = "tab-\(names[index])"
        return item
      }
      if surface.tabs.items?.map(\.title) != items.map(\.title) {
        let pages = items.map { item in
          let page = UIViewController()
          page.view.backgroundColor = .clear
          page.view.isOpaque = false
          page.tabBarItem = item
          return page
        }
        surface.tabController.setViewControllers(pages, animated: false)
      }
      let currentItems = surface.tabs.items ?? items
      let selected = (next["selected"] as? NSNumber)?.intValue ?? 0
      let target = currentItems.indices.contains(selected) ? selected : 0
      // A tap has already selected its controller. Do not start it again.
      if surface.tabController.selectedIndex != target {
        UIView.performWithoutAnimation {
          surface.tabController.selectedIndex = target
          surface.tabController.view.layoutIfNeeded()
        }
      }
    } else {
      let item = UINavigationItem(title: next["title"] as? String ?? "")
      let back = next["back"] as? Bool == true
      let large = next["largeTitle"] as? Bool == true && !back
      surface.navigation.prefersLargeTitles = large
      item.largeTitleDisplayMode = large ? .always : .never
      let actions = next["actions"] as? [[String: Any]] ?? []
      item.rightBarButtonItems = actions.reversed().compactMap { action -> UIBarButtonItem? in
        guard let id = action["id"] as? String, let label = action["label"] as? String else { return nil }
        let symbol = (action["symbol"] as? String).flatMap { $0 == "bell" ? materialImage(57345) : nil }
        let command = UIAction(title: label, image: symbol) { [weak self] _ in self?.emit(id) }
        let button = UIBarButtonItem(title: symbol == nil ? label : nil, image: symbol,
          primaryAction: command, menu: nil)
        button.tintColor = accent
        button.accessibilityLabel = label
        button.accessibilityIdentifier = id
        button.isEnabled = action["enabled"] as? Bool ?? true
        if action["prominent"] as? Bool == true { button.style = .prominent }
        if let badge = (action["badge"] as? NSNumber)?.intValue, badge > 0 {
          button.badge = .count(badge)
        }
        return button
      }
      if back {
        let previous = UINavigationItem(title: next["backLabel"] as? String ?? "")
        surface.navigation.setItems([previous, item], animated: false)
      } else {
        surface.navigation.setItems([item], animated: false)
      }
    }
    surface.setNeedsLayout()
  }

  func tabBarController(_ tabBarController: UITabBarController, didSelect viewController: UIViewController) {
    emit("tab:\(tabBarController.selectedIndex)")
  }

  func navigationBar(_ navigationBar: UINavigationBar, shouldPop item: UINavigationItem) -> Bool {
    emit("back")
    return false // Flutter owns the route; it disposes this view after popping.
  }
}

@available(iOS 26.0, *)
@MainActor
private final class ChromeSurface: UIView {
  let kind: String
  let navigation = UINavigationBar()
  let tabController = UITabBarController()
  var tabs: UITabBar { tabController.tabBar }
  var topInset: CGFloat = 0

  init(frame: CGRect, kind: String) {
    self.kind = kind
    super.init(frame: frame)
    backgroundColor = .clear
    isOpaque = false
    navigation.isOpaque = false
    navigation.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16)
    // UIKit supplies Liquid Glass. Do not replace its appearance/background.
    // This embedded controller owns only the existing bottom phone tabs.
    tabController.traitOverrides.horizontalSizeClass = .compact
    tabController.view.backgroundColor = .clear
    tabController.view.isOpaque = false
    addSubview(kind == "tabs" ? tabController.view : navigation)
  }
  required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }

  override func didMoveToWindow() {
    super.didMoveToWindow()
    guard kind == "tabs" else { return }
    // Flutter detaches platform-view surfaces while a route is covered.
    // Keep the same child controller so UIKit does not replay tab selection.
    if window != nil && tabController.parent == nil {
      var responder: UIResponder? = next
      while let current = responder {
        if let parent = current as? UIViewController {
          parent.addChild(tabController)
          tabController.didMove(toParent: parent)
          break
        }
        responder = current.next
      }
    }
  }

  override func layoutSubviews() {
    super.layoutSubviews()
    if kind == "tabs" {
      tabController.view.frame = bounds
    } else {
      navigation.frame = CGRect(x: 0, y: topInset, width: bounds.width, height: max(0, bounds.height - topInset))
    }
  }
}
