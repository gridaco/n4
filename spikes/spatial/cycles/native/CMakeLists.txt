# Built inside the Cycles tree (see cycles/patches) so it inherits Cycles' targets and settings.
add_library(n4cycles SHARED n4_cycles.cpp n4_cycles.h)
target_include_directories(n4cycles PRIVATE ${CMAKE_SOURCE_DIR}/src)
target_link_libraries(n4cycles PRIVATE ${LIB})
set_target_properties(n4cycles PROPERTIES
  CXX_VISIBILITY_PRESET hidden
  INSTALL_NAME_DIR "@rpath")
install(TARGETS n4cycles DESTINATION ${CMAKE_INSTALL_PREFIX})
