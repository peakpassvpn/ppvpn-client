# window_manager 0.5.2: hidden sizing/style changes must not activate the app.
# Compile only this copied source; the shared package cache stays untouched.
function(ppvpn_materialize_window_manager source_dir output_dir)
  set(source "${source_dir}/window_manager.cpp")
  file(SHA256 "${source}" original_sha)
  if(NOT original_sha STREQUAL "fe90a377f8d14b37f5643e7db4550ea0fb64a54d8f0a9751cb7ae10744860b53")
    message(FATAL_ERROR "window_manager 0.5.2 native source differs from the pinned package")
  endif()
  file(SHA256 "${source_dir}/window_manager_plugin.cpp" plugin_sha)
  if(NOT plugin_sha STREQUAL "6fa2dc3eea3543108957a142e273a4fc9faf101bae09444bcd9be4712d3238d0")
    message(FATAL_ERROR "window_manager 0.5.2 plugin source differs from the pinned package")
  endif()
  file(READ "${source}" code)
  set(old_bounds "SetWindowPos(hwnd, HWND_TOP, x, y, width, height, uFlags);")
  set(new_bounds "SetWindowPos(hwnd, HWND_TOP, x, y, width, height, uFlags | (IsWindowVisible(hwnd) ? 0 : SWP_NOACTIVATE));")
  string(FIND "${code}" "${old_bounds}" bounds_at)
  if(bounds_at LESS 0)
    message(FATAL_ERROR "window_manager SetBounds patch does not match")
  endif()
  string(REPLACE "${old_bounds}" "${new_bounds}" code "${code}")
  set(old_style [=[  DwmExtendFrameIntoClientArea(hWnd, &margins);
  SetWindowPos(hWnd, nullptr, rect.left, rect.top, 0, 0,
               SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOMOVE | SWP_NOSIZE |
                   SWP_FRAMECHANGED);]=])
  set(new_style [=[  DwmExtendFrameIntoClientArea(hWnd, &margins);
  SetWindowPos(hWnd, nullptr, rect.left, rect.top, 0, 0,
               SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_NOMOVE | SWP_NOSIZE |
                   SWP_FRAMECHANGED | (IsWindowVisible(hWnd) ? 0 : SWP_NOACTIVATE));]=])
  string(FIND "${code}" "${old_style}" style_at)
  if(style_at LESS 0)
    message(FATAL_ERROR "window_manager SetTitleBarStyle patch does not match")
  endif()
  string(REPLACE "${old_style}" "${new_style}" code "${code}")
  file(MAKE_DIRECTORY "${output_dir}")
  file(WRITE "${output_dir}/window_manager.cpp" "${code}")
  # plugin.cpp includes window_manager.cpp, so both original translation
  # units must come from this copy or the used implementation stays unpatched.
  # The engine's framework-origin Focus() calls SetFocus even for a hidden
  # view. Windows rejects focus to a disabled child; restore input before the
  # real parent show so explicit opens keep the original view and keyboard.
  file(READ "${source_dir}/window_manager_plugin.cpp" plugin_code)
  set(old_init [=[    window_manager->native_window =
        ::GetAncestor(registrar->GetView()->GetNativeWindow(), GA_ROOT);]=])
  set(new_init [=[    window_manager->native_window =
        ::GetAncestor(registrar->GetView()->GetNativeWindow(), GA_ROOT);
    ::EnableWindow(registrar->GetView()->GetNativeWindow(),
                   ::IsWindowVisible(window_manager->native_window));]=])
  set(old_show "  } else if (message == WM_SHOWWINDOW) {")
  set(new_show [=[  } else if (message == WM_SHOWWINDOW) {
    if (hWnd == window_manager->native_window) {
      ::EnableWindow(registrar->GetView()->GetNativeWindow(), wParam == TRUE);
    }]=])
  foreach(site IN ITEMS init show)
    string(FIND "${plugin_code}" "${old_${site}}" site_at)
    if(site_at LESS 0)
      message(FATAL_ERROR "window_manager hidden view input patch does not match ${site}")
    endif()
    string(REPLACE "${old_${site}}" "${new_${site}}" plugin_code "${plugin_code}")
  endforeach()
  file(WRITE "${output_dir}/window_manager_plugin.cpp" "${plugin_code}")
  # The original source uses a quoted include/window_manager/... header.
  file(COPY "${source_dir}/include" DESTINATION "${output_dir}")
  file(SHA256 "${output_dir}/window_manager.cpp" patched_sha)
  file(SHA256 "${output_dir}/window_manager_plugin.cpp" patched_plugin_sha)
  file(WRITE "${output_dir}/source-identity.txt" "package=window_manager 0.5.2\noriginal_sha256=${original_sha}\npatched_sha256=${patched_sha}\nplugin_source_sha256=${plugin_sha}\npatched_plugin_sha256=${patched_plugin_sha}\n")
endfunction()

# Source materialization can be checked without configuring or compiling an app.
if(CMAKE_SCRIPT_MODE_FILE)
  ppvpn_materialize_window_manager("${PPVPN_WINDOW_MANAGER_SOURCE_DIR}" "${PPVPN_WINDOW_MANAGER_OUTPUT_DIR}")
  return()
endif()

if(NOT TARGET window_manager_plugin)
  message(FATAL_ERROR "Expected the existing window_manager_plugin target")
endif()
get_target_property(ppvpn_wm_source_dir window_manager_plugin SOURCE_DIR)
get_target_property(ppvpn_wm_sources window_manager_plugin SOURCES)
set(ppvpn_wm_output "${CMAKE_CURRENT_BINARY_DIR}/ppvpn_window_manager")
ppvpn_materialize_window_manager("${ppvpn_wm_source_dir}" "${ppvpn_wm_output}")
set(ppvpn_wm_compiled_sources "")
set(ppvpn_wm_replaced 0)
foreach(source IN LISTS ppvpn_wm_sources)
  if(IS_ABSOLUTE "${source}")
    set(absolute_source "${source}")
  else()
    set(absolute_source "${ppvpn_wm_source_dir}/${source}")
  endif()
  if(absolute_source STREQUAL "${ppvpn_wm_source_dir}/window_manager.cpp" OR absolute_source STREQUAL "${ppvpn_wm_source_dir}/window_manager_plugin.cpp")
    get_filename_component(filename "${absolute_source}" NAME)
    list(APPEND ppvpn_wm_compiled_sources "${ppvpn_wm_output}/${filename}")
    math(EXPR ppvpn_wm_replaced "${ppvpn_wm_replaced}+1")
  else()
    list(APPEND ppvpn_wm_compiled_sources "${absolute_source}")
  endif()
endforeach()
if(NOT ppvpn_wm_replaced EQUAL 2)
  message(FATAL_ERROR "Expected both original window_manager native target sources")
endif()
set_property(TARGET window_manager_plugin PROPERTY SOURCES "${ppvpn_wm_compiled_sources}")
