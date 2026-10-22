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
        auto *scene=root.createSceneManager();
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
        root.renderOneFrame();
        std::vector<unsigned char> pixels(64*64*4);
        Ogre::PixelBox box(64,64,1,Ogre::PF_BYTE_RGBA,pixels.data());
        window->copyContentsToMemory(Ogre::Box(0,0,64,64),box,Ogre::RenderTarget::FB_AUTO);
        auto offset=(32*64+32)*4;
        if(pixels[offset]<240 || pixels[offset+1]>10 || pixels[offset+2]>10) return 3;
        unsigned long xid=0;window->getCustomAttribute("WINDOW",&xid);
        std::cout<<"GLX_READY "<<xid<<" OGRE "<<OGRE_VERSION_NAME<<std::endl;
        (void)std::cin.get();
        return 0;
    } catch(const std::exception &error) {std::cerr<<error.what()<<std::endl;return 4;}
}
