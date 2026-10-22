shared_library('wayx-xserver',
    ['wayx-output.c', 'wayx-input.c', 'wayx-glx.c', '../../mi/miinitext.c', '../stubs/ddxBeforeReset.c'],
    include_directories: [inc, include_directories('.', 'gl-headers')],
    dependencies: common_dep,
    link_with: [libxserver_fb, libxserver, libxserver_xkeyboard_stubs,
                libxserver_xi_stubs, libxserver_glx, libglxvnd],
    install: false,
)
