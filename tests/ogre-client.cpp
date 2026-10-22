// SPDX-License-Identifier: MIT
// Exercise the renderer family used by RViz, over the same TCP X11 display.
#include <Ogre.h>
#include <cstdlib>
#include <iostream>
#include <vector>
int main() {
    try {
        Ogre::Root root("", "", "");
        const char *plugin=std::getenv("WAYX_TEST_OGRE_PLUGIN");
        root.loadPlugin(plugin?plugin:"RenderSystem_GL");
        auto *renderer=root.getRenderSystemByName("OpenGL Rendering Subsystem");
        if(!renderer) return 2;
        root.setRenderSystem(renderer);root.initialise(false);
        Ogre::NameValuePairList options{{"left","20"},{"top","20"},{"border","none"},{"FSAA","0"}};
        auto *window=root.createRenderWindow("WayX OGRE validation",64,64,false,&options);
#if OGRE_VERSION < ((1 << 16) | (10 << 8))
        auto *scene=root.createSceneManager(Ogre::ST_GENERIC,"scene");
#else
        auto *scene=root.createSceneManager();
#endif
        auto *camera=scene->createCamera("camera");
        camera->setProjectionType(Ogre::PT_ORTHOGRAPHIC);camera->setOrthoWindow(2,2);
        camera->setNearClipDistance(.1);camera->setFarClipDistance(10);
        auto *node=scene->getRootSceneNode()->createChildSceneNode();
        node->setPosition(0,0,2);node->attachObject(camera);
        auto *viewport=window->addViewport(camera);viewport->setBackgroundColour(Ogre::ColourValue(0,0,1));
        auto material=Ogre::MaterialManager::getSingleton().create("red",Ogre::ResourceGroupManager::DEFAULT_RESOURCE_GROUP_NAME);
        material->getTechnique(0)->getPass(0)->setLightingEnabled(false);
        material->getTechnique(0)->getPass(0)->setVertexColourTracking(Ogre::TVC_DIFFUSE);
        material->getTechnique(0)->getPass(0)->setCullingMode(Ogre::CULL_NONE);
        auto *triangle=scene->createManualObject();
        triangle->begin("red",Ogre::RenderOperation::OT_TRIANGLE_LIST);
        triangle->position(-.8,-.8,0);triangle->colour(1,0,0);
        triangle->position(.8,-.8,0);triangle->colour(1,0,0);
        triangle->position(0,.8,0);triangle->colour(1,0,0);triangle->end();
        scene->getRootSceneNode()->createChildSceneNode()->attachObject(triangle);
        window->setActive(true);window->setVisible(true);
        window->update(false);
        std::vector<unsigned char> pixels(64*64*4);
        Ogre::PixelBox box(64,64,1,Ogre::PF_BYTE_RGBA,pixels.data());
        // Read the rendered back buffer before swapping: its contents after
        // a swap are undefined, notably with Mesa 18 and OGRE 1.9.
#if OGRE_VERSION < ((1 << 16) | (10 << 8))
        window->copyContentsToMemory(box,Ogre::RenderTarget::FB_BACK);
#else
        window->copyContentsToMemory(Ogre::Box(0,0,64,64),box,Ogre::RenderTarget::FB_BACK);
#endif
        auto offset=(32*64+32)*4;
        if(pixels[offset]<240 || pixels[offset+1]>10 || pixels[offset+2]>10) return 3;
        window->swapBuffers();
        unsigned long xid=0;window->getCustomAttribute("WINDOW",&xid);
        std::cout<<"GLX_READY "<<xid<<" OGRE "<<OGRE_VERSION_NAME<<std::endl;
        (void)std::cin.get();
        return 0;
    } catch(const std::exception &error) {std::cerr<<error.what()<<std::endl;return 4;}
}
