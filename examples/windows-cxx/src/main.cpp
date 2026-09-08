#include <exception>
#include <iostream>
#include <stdexcept>
#include <string>

int main() {
    try {
        throw std::runtime_error("rcc-cxx");
    } catch (const std::runtime_error &error) {
        if (std::string(error.what()) != "rcc-cxx") {
            std::cerr << "exception path failed\n";
            return 1;
        }
    }
    std::cout << "rcc-cxx-ok iostream exception\n";
    return 0;
}
